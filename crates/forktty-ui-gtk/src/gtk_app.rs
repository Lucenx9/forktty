use adw::prelude::*;
use forktty_core::{
    config, dispatch_notification, session, worktree, LogLevel, NotificationItem, NotificationKind,
    PaneNode, ProgressEntry, SplitAxis, StatusEntry, Surface, WorkspaceModel, WorkspaceSelector,
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
use std::process::Command;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.forktty.ForkTTY";
const DEFAULT_FONT_FAMILY_ID: &str = "__forktty_default_font__";
const SYSTEM_MONOSPACE_FONT_FAMILY_ID: &str = "__forktty_system_monospace__";
const PREFERRED_TERMINAL_FONT_FAMILIES: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "FantasqueSansM Nerd Font Mono",
    "FiraCode Nerd Font Mono",
    "Hack Nerd Font Mono",
    "Iosevka Nerd Font Mono",
    "Symbols Nerd Font Mono",
];
const PROMPT_NOTIFICATION_THROTTLE: Duration = Duration::from_secs(8);
const NOTIFICATION_DEDUPE_WINDOW: Duration = Duration::from_secs(12);
const PANED_RATIO_APPLY_FRAMES: u8 = 8;
const PANED_RATIO_MAX_FRAMES: u8 = 30;
const SPLIT_VERTICAL_SHORTCUT: &str = "Ctrl+Shift+E";
const SPLIT_VERTICAL_ACCEL: &str = "<Control><Shift>E";
const RESTART_PANE_SHORTCUT: &str = "Ctrl+Shift+R";
const RESTART_PANE_ACCEL: &str = "<Control><Shift>R";
const EMPTY_LAYOUT_SIGNATURE: &str = "empty-layout";

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
    single_pane_actions: gtk::Box,
    focus_marker: gtk::Box,
    title: gtk::Label,
    cwd: gtk::Label,
    attention_dot: gtk::Box,
}

struct SidebarWorkspaceRow {
    workspace: forktty_core::Workspace,
    meta: String,
    summary: String,
    status: Option<WorkspaceStatusBadge>,
    surface_count: usize,
}

#[derive(Clone)]
struct WorkspaceStatusBadge {
    label: &'static str,
    class_name: &'static str,
}

struct SidebarSnapshot {
    rows: Vec<SidebarWorkspaceRow>,
    active_workspace_name: Option<String>,
    active_status_label: Option<String>,
    active_pane_label: Option<String>,
    signature: String,
}

#[derive(Clone)]
struct SidebarUi {
    sidebar: gtk::ListBox,
    parent_window: adw::ApplicationWindow,
    workspace_title: gtk::Button,
    status_location: gtk::Button,
    pane_status: gtk::Label,
    last_signature: Rc<RefCell<Option<String>>>,
    context_menu_open: Rc<Cell<bool>>,
    context_popover: Rc<RefCell<Option<gtk::Popover>>>,
}

type SplitResizeCallback = Rc<dyn Fn(&[String], &[String], f64)>;
type SettingsApplyCallback = Rc<dyn Fn(&config::AppConfig)>;

struct VteController {
    container: gtk::Box,
    parent_window: adw::ApplicationWindow,
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
    widgets: BTreeMap<String, VteTerminalWidget>,
    chromes: BTreeMap<String, PaneChrome>,
    pending_spawns: BTreeSet<String>,
    last_layout_signature: Option<String>,
    maximized_pane: bool,
}

impl VteController {
    fn new(
        container: gtk::Box,
        parent_window: adw::ApplicationWindow,
        model: Arc<Mutex<WorkspaceModel>>,
    ) -> Self {
        Self {
            container,
            parent_window,
            model,
            state: None,
            widgets: BTreeMap::new(),
            chromes: BTreeMap::new(),
            pending_spawns: BTreeSet::new(),
            last_layout_signature: None,
            maximized_pane: false,
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
        let spawn_model_for_error = spawn_model.clone();
        match spawn_vte_terminal_with_callback(&request, move |result| {
            if let Err(err) = result {
                record_terminal_spawn_failure(
                    &spawn_model,
                    &spawn_workspace_id,
                    &spawn_surface_id,
                    &err.to_string(),
                );
            }
        }) {
            Ok(widget) => {
                if let Ok(mut model) = self.model.lock() {
                    let _ = model.clear_status(
                        &request.workspace_id,
                        Some(&surface_status_key(&request.surface_id)),
                    );
                }
                apply_vte_appearance(&widget);
                attach_vte_signal_handlers(&widget, &self.model, &request);
                widget.grab_focus();
                let chrome = build_pane_chrome(
                    &request.surface_id,
                    &widget,
                    self.state.as_ref(),
                    &self.parent_window,
                );
                self.chromes.insert(request.surface_id.clone(), chrome);
                self.widgets.insert(request.surface_id, widget);
                self.rebuild_layout();
            }
            Err(err) => {
                record_terminal_spawn_failure(
                    &spawn_model_for_error,
                    &request.workspace_id,
                    &request.surface_id,
                    &err.to_string(),
                );
                if let Some(state) = &self.state {
                    let _ = state.terminal.close(&request.surface_id);
                }
                self.last_layout_signature = None;
                self.rebuild_layout();
            }
        }
    }

    fn rebuild_layout(&mut self) {
        self.spawn_active_surfaces_if_needed();
        for chrome in self.chromes.values() {
            detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
        }
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }

        let Some((signature, pane_tree, focused_surface_id, workspace_id)) =
            active_layout_snapshot(&self.model)
        else {
            self.last_layout_signature = Some(EMPTY_LAYOUT_SIGNATURE.to_string());
            self.container.append(&empty_terminal_stage(
                self.state.as_ref(),
                Some(&self.parent_window),
            ));
            return;
        };
        let visible_tree = if self.maximized_pane {
            PaneNode::Leaf {
                surface_id: focused_surface_id.clone(),
            }
        } else {
            pane_tree
        };
        let widget = self.widget_for_pane(&visible_tree, &workspace_id);
        let single_pane = collect_leaves(&visible_tree).len() == 1;
        for chrome in self.chromes.values() {
            chrome.header.set_visible(!single_pane);
            chrome.single_pane_actions.set_visible(single_pane);
            chrome.single_pane_actions.set_sensitive(single_pane);
        }
        self.container.append(&widget);
        if let Some(widget) = self.widgets.get(&focused_surface_id) {
            widget.grab_focus();
        }
        self.last_layout_signature = Some(signature);
    }

    fn toggle_maximized_pane(&mut self) {
        self.maximized_pane = !self.maximized_pane;
        self.last_layout_signature = None;
        self.rebuild_layout();
    }

    fn ensure_layout_current(&mut self) {
        self.spawn_active_surfaces_if_needed();
        let Some((signature, _, _, _)) = active_layout_snapshot(&self.model) else {
            if self.last_layout_signature.as_deref() != Some(EMPTY_LAYOUT_SIGNATURE) {
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
            let statuses = model.list_status(&workspace.id);
            model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| {
                    let blocked = surface_status_blocks_auto_spawn(&statuses, &surface.id);
                    (surface, blocked)
                })
                .collect::<Vec<_>>()
        };
        for (surface, auto_spawn_blocked) in surfaces {
            if auto_spawn_blocked {
                continue;
            }
            if self.widgets.contains_key(&surface.id)
                || self.pending_spawns.contains(&surface.id)
                || backend_surface_ids.contains(&surface.id)
            {
                continue;
            }
            let surface_id = surface.id.clone();
            let workspace_id = surface.workspace_id.clone();
            self.pending_spawns.insert(surface_id.clone());
            if let Err(err) = state.terminal.spawn(SpawnRequest {
                surface_id: surface_id.clone(),
                workspace_id: workspace_id.clone(),
                shell: state.shell.clone(),
                cwd: surface.cwd.clone(),
                socket_path: state.socket_path.clone(),
                extra_env: Vec::new(),
            }) {
                self.pending_spawns.remove(&surface_id);
                record_terminal_spawn_failure(
                    &self.model,
                    &workspace_id,
                    &surface_id,
                    &err.to_string(),
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
        let on_resize: SplitResizeCallback =
            Rc::new(move |left: &[String], right: &[String], ratio: f64| {
                if let Ok(mut model) = model.lock() {
                    let _ = model.update_split_partition_ratio(
                        &workspace_id_for_resize,
                        left,
                        right,
                        ratio,
                    );
                }
            });
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
            return missing_surface_placeholder(surface_id, self.state.as_ref(), Some(&self.model))
                .upcast();
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
    parent: &adw::ApplicationWindow,
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
    attention_dot.update_property(&[gtk::accessible::Property::Label("Pane needs attention")]);
    let focus_marker = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    focus_marker.add_css_class("pane-focus-marker");
    focus_marker.set_size_request(8, 8);
    focus_marker.set_valign(gtk::Align::Center);
    focus_marker.set_visible(false);
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
    let split_h = pane_action_button("view-dual-symbolic", "Split Right (Ctrl+Shift+H)");
    let split_v = pane_action_button(
        "view-paged-symbolic",
        &format!("Split Down ({SPLIT_VERTICAL_SHORTCUT})"),
    );
    let close = pane_action_button("window-close-symbolic", "Close Pane (Ctrl+Shift+W)");
    close.add_css_class("pane-close-action");
    let close_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    close_separator.add_css_class("pane-action-separator");
    actions.append(&split_h);
    actions.append(&split_v);
    actions.append(&close_separator);
    actions.append(&close);

    let terminal_overlay = gtk::Overlay::new();
    terminal_overlay.set_hexpand(true);
    terminal_overlay.set_vexpand(true);
    terminal_overlay.set_child(Some(widget));

    let single_pane_actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    single_pane_actions.add_css_class("single-pane-actions");
    single_pane_actions.set_halign(gtk::Align::End);
    single_pane_actions.set_valign(gtk::Align::Start);
    single_pane_actions.set_visible(false);
    single_pane_actions.set_sensitive(false);
    let single_split_h = pane_action_button("view-dual-symbolic", "Split Right (Ctrl+Shift+H)");
    let single_split_v = pane_action_button(
        "view-paged-symbolic",
        &format!("Split Down ({SPLIT_VERTICAL_SHORTCUT})"),
    );
    single_pane_actions.append(&single_split_h);
    single_pane_actions.append(&single_split_v);
    terminal_overlay.add_overlay(&single_pane_actions);

    if let Some(state) = state {
        install_terminal_context_menu(widget, surface_id, state, parent);
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
        let state_for_single_h = state.clone();
        let sid_single_h = surface_id_owned.clone();
        single_split_h.connect_clicked(move |_| {
            focus_surface_and(&state_for_single_h, &sid_single_h, |s| {
                split_active_surface(s, SplitAxis::Horizontal)
            });
        });
        let state_for_single_v = state.clone();
        let sid_single_v = surface_id_owned.clone();
        single_split_v.connect_clicked(move |_| {
            focus_surface_and(&state_for_single_v, &sid_single_v, |s| {
                split_active_surface(s, SplitAxis::Vertical)
            });
        });
        let state_for_c = state.clone();
        let parent_for_c = parent.clone();
        let sid_c = surface_id_owned;
        close.connect_clicked(move |_| {
            show_close_pane_confirmation(&parent_for_c, &state_for_c, &sid_c);
        });
    } else {
        split_h.set_sensitive(false);
        split_v.set_sensitive(false);
        single_split_h.set_sensitive(false);
        single_split_v.set_sensitive(false);
        close.set_sensitive(false);
    }

    let actions_hovered = Rc::new(Cell::new(false));
    let actions_focused = Rc::new(Cell::new(false));
    let motion = gtk::EventControllerMotion::new();
    {
        let actions_for_enter = actions.clone();
        let actions_hovered = actions_hovered.clone();
        motion.connect_enter(move |_, _, _| {
            actions_hovered.set(true);
            actions_for_enter.set_sensitive(true);
            actions_for_enter.add_css_class("revealed");
        });
    }
    {
        let actions_for_leave = actions.clone();
        let actions_hovered = actions_hovered.clone();
        let actions_focused = actions_focused.clone();
        motion.connect_leave(move |_| {
            actions_hovered.set(false);
            actions_for_leave.remove_css_class("revealed");
            if !actions_focused.get() {
                actions_for_leave.set_sensitive(false);
            }
        });
    }
    header.add_controller(motion);

    let focus = gtk::EventControllerFocus::new();
    {
        let actions_for_focus = actions.clone();
        let actions_focused = actions_focused.clone();
        focus.connect_enter(move |_| {
            actions_focused.set(true);
            actions_for_focus.set_sensitive(true);
            actions_for_focus.add_css_class("focus-revealed");
        });
    }
    {
        let actions_for_focus = actions.clone();
        let actions_hovered = actions_hovered.clone();
        let actions_focused = actions_focused.clone();
        focus.connect_leave(move |_| {
            actions_focused.set(false);
            actions_for_focus.remove_css_class("focus-revealed");
            if !actions_hovered.get() {
                actions_for_focus.set_sensitive(false);
            }
        });
    }
    actions.add_controller(focus);

    header.append(&focus_marker);
    header.append(&attention_dot);
    header.append(&title);
    header.append(&cwd);
    header.append(&actions);
    pane.append(&header);
    pane.append(&terminal_overlay);

    PaneChrome {
        pane,
        header,
        single_pane_actions,
        focus_marker,
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

fn focused_surface_id(state: &SocketAppState) -> Option<String> {
    let model = state.model.lock().ok()?;
    model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.active)
        .or_else(|| model.list_workspaces().into_iter().next())
        .map(|workspace| workspace.focused_surface_id)
}

fn show_close_pane_confirmation(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    surface_id: &str,
) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    show_destructive_confirmation(
        parent,
        "Close Pane?",
        "Close this terminal pane. Any process running inside it will be terminated.",
        "Close Pane",
        move || {
            focus_surface_and(&state, &surface_id, close_active_surface);
        },
    );
}

fn record_terminal_spawn_failure(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    message: &str,
) {
    if let Ok(mut model) = model.lock() {
        let value = if message.trim().is_empty() {
            "Spawn failed".to_string()
        } else {
            format!("Spawn failed: {}", truncate_single_line(message, 140))
        };
        let _ = model.set_status(
            workspace_id,
            surface_status_key(surface_id),
            "Terminal",
            value,
            Some("red".to_string()),
        );
        let _ = model.append_log(
            workspace_id,
            LogLevel::Error,
            format!("Terminal {surface_id} spawn failed: {message}"),
        );
        let notification = model.create_notification(
            "Terminal spawn failed",
            message,
            NotificationKind::Error,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        );
        dispatch_notification_with_loaded_config(&notification);
    }
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
    chrome.focus_marker.set_visible(active);
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
    if terminal_prefers_dark_palette(&config) {
        widget.set_color_background(&rgba("#1c1d2a"));
        widget.set_color_foreground(&rgba("#cdd6f4"));
        widget.set_color_bold(Some(&rgba("#f5f6fb")));
        widget.set_color_cursor(Some(&rgba("#89b4fa")));
        widget.set_color_cursor_foreground(Some(&rgba("#0a0c12")));
        widget.set_color_highlight(Some(&rgba("#2f3146")));
        widget.set_color_highlight_foreground(Some(&rgba("#f5f6fb")));
    } else {
        widget.set_color_background(&rgba("#fbfbfe"));
        widget.set_color_foreground(&rgba("#1f2328"));
        widget.set_color_bold(Some(&rgba("#0b0d12")));
        widget.set_color_cursor(Some(&rgba("#1f6feb")));
        widget.set_color_cursor_foreground(Some(&rgba("#ffffff")));
        widget.set_color_highlight(Some(&rgba("#dbeafe")));
        widget.set_color_highlight_foreground(Some(&rgba("#111827")));
    }
}

fn terminal_prefers_dark_palette(config: &config::AppConfig) -> bool {
    let source = config.general.theme_source.trim().to_ascii_lowercase();
    match source.as_str() {
        "light" => false,
        "dark" => true,
        _ => adw::StyleManager::default().is_dark(),
    }
}

fn terminal_font_description(config: &config::AppConfig) -> gtk::pango::FontDescription {
    let configured = config.appearance.font_family.trim();
    let family = if configured.is_empty() {
        default_terminal_font_family(&installed_font_families())
    } else {
        configured.to_string()
    };
    terminal_font_description_with_family(config, family)
}

fn terminal_font_description_with_family(
    config: &config::AppConfig,
    default_family: String,
) -> gtk::pango::FontDescription {
    let configured = config.appearance.font_family.trim();
    let family = if configured.is_empty() {
        default_family
    } else {
        configured.to_string()
    };
    gtk::pango::FontDescription::from_string(&format!("{} {}", family, config.appearance.font_size))
}

fn rgba(value: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(value).unwrap_or(gtk::gdk::RGBA::BLACK)
}

fn default_terminal_font_family(installed_families: &[String]) -> String {
    for preferred in PREFERRED_TERMINAL_FONT_FAMILIES {
        if installed_families
            .iter()
            .any(|family| family.eq_ignore_ascii_case(preferred))
        {
            return preferred.to_string();
        }
    }
    installed_families
        .iter()
        .find(|family| family.eq_ignore_ascii_case("monospace"))
        .cloned()
        .unwrap_or_else(|| "monospace".to_string())
}

fn installed_font_families() -> Vec<String> {
    fontconfig_list_families(":").unwrap_or_default()
}

fn installed_monospace_font_families() -> Vec<String> {
    fontconfig_list_families(":spacing=mono").unwrap_or_else(installed_font_families)
}

fn fontconfig_list_families(pattern: &str) -> Option<Vec<String>> {
    let output = Command::new("fc-list")
        .args([pattern, "family"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_fontconfig_families(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn resolved_system_monospace_family() -> Option<String> {
    let output = Command::new("fc-match")
        .args(["-f", "%{family}\n", "monospace"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    first_fontconfig_family(&String::from_utf8_lossy(&output.stdout))
}

fn parse_fontconfig_families(output: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in output.lines() {
        for name in line.split(',') {
            let name = name.trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    names.into_iter().collect()
}

fn first_fontconfig_family(output: &str) -> Option<String> {
    output
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(ToOwned::to_owned)
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
            let _ = model.set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Terminal",
                format!("Exited ({status})"),
                Some("yellow".to_string()),
            );
            let notification = model.create_notification(
                "Terminal exited",
                format!("Process exited with status {status}. Use Restart Pane to spawn it again."),
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

fn labeled_icon_button_parts(
    icon_name: &str,
    label: &str,
) -> (gtk::Button, gtk::Image, gtk::Label) {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(icon_name);
    let label_widget = gtk::Label::new(Some(label));
    content.append(&icon);
    content.append(&label_widget);
    let button = gtk::Button::builder().child(&content).build();
    set_accessible_button_text(&button, label, None);
    (button, icon, label_widget)
}

fn apply_dialog_chrome(dialog: &gtk::Window) {
    let titlebar = gtk::HeaderBar::new();
    titlebar.set_show_title_buttons(false);
    titlebar.add_css_class("ft-dialog-titlebar");
    titlebar.set_title_widget(Some(&gtk::Label::new(None)));
    let close = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Close")
        .build();
    close.add_css_class("flat");
    close.add_css_class("ft-dialog-close");
    set_accessible_button_text(&close, "Close", Some("Esc"));
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    titlebar.pack_end(&close);
    dialog.set_titlebar(Some(&titlebar));
}

fn restore_focus_after_hide<W>(dialog: &gtk::Window, parent: &W)
where
    W: IsA<gtk::Window>,
{
    let previous_focus: Option<gtk::Widget> = gtk::prelude::GtkWindowExt::focus(parent.as_ref());
    dialog.connect_hide(move |_| {
        if let Some(widget) = previous_focus.as_ref() {
            if widget.root().is_some() {
                widget.grab_focus();
            }
        }
    });
}

fn install_escape_close(window: &gtk::Window) {
    let controller = gtk::EventControllerKey::new();
    let window_for_close = window.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let is_close_shortcut =
            key == gtk::gdk::Key::w && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if key == gtk::gdk::Key::Escape || is_close_shortcut {
            window_for_close.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

fn show_destructive_confirmation<W, F>(
    parent: &W,
    title: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: F,
) where
    W: IsA<gtk::Window>,
    F: Fn() + 'static,
{
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(380)
        .default_height(160)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title_label = gtk::Label::builder().label(title).xalign(0.0).build();
    title_label.add_css_class("ft-dialog-title");
    let body_label = gtk::Label::builder()
        .label(body)
        .xalign(0.0)
        .wrap(true)
        .build();
    body_label.add_css_class("ft-dialog-subtitle");
    header.append(&title_label);
    header.append(&body_label);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let confirm = gtk::Button::with_label(confirm_label);
    confirm.add_css_class("destructive-action");
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&confirm);

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());
    let dialog_for_confirm = dialog.clone();
    confirm.connect_clicked(move |_| {
        on_confirm();
        dialog_for_confirm.close();
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&footer);
    dialog.set_default_widget(Some(&cancel));
    dialog.set_child(Some(&content));
    dialog.present();
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

fn clear_status_message(label: &gtk::Label) {
    label.set_text("");
    label.set_visible(false);
    label.remove_css_class("success");
    label.remove_css_class("error");
}

fn refresh_notification_indicator(button: &gtk::Button, state: &SocketAppState) {
    let count = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications().len())
        .unwrap_or(0);
    if count == 0 {
        button.remove_css_class("needs-attention");
        button.set_tooltip_text(Some("Notifications (Ctrl+Shift+M)"));
        set_accessible_button_text(button, "Notifications", Some("Ctrl+Shift+M"));
    } else {
        button.add_css_class("needs-attention");
        let label = if count == 1 {
            "Notifications: 1 pending".to_string()
        } else {
            format!("Notifications: {count} pending")
        };
        button.set_tooltip_text(Some(&format!("{label} (Ctrl+Shift+M)")));
        set_accessible_button_text(button, &label, Some("Ctrl+Shift+M"));
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

fn copy_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
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
        "Split Right",
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
        "Split Down",
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
            copy_to_clipboard(&path_text);
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
        let parent_ = parent.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "user-trash-symbolic",
            "Remove Worktree",
            true,
            move || {
                let state_confirm = state_.clone();
                let name_confirm = name_.clone();
                show_destructive_confirmation(
                    &parent_,
                    "Remove Worktree?",
                    &format!(
                        "Remove worktree '{name_confirm}' and close its ForkTTY workspace. This cannot be undone from ForkTTY."
                    ),
                    "Remove Worktree",
                    move || {
                        if let Err(err) = remove_worktree_from_gtk(&state_confirm, &name_confirm) {
                            create_local_notification(&state_confirm, "Remove Failed", &err);
                        }
                    },
                );
            },
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Workspace",
        true,
        move || {
            let state_confirm = state_.clone();
            let ws_id_confirm = ws_id.clone();
            show_destructive_confirmation(
                &parent_,
                "Close Workspace?",
                "Close this workspace and all panes inside it. Running terminal processes in this workspace will be closed.",
                "Close Workspace",
                move || {
                    focus_workspace(&state_confirm, &ws_id_confirm);
                    close_active_workspace(&state_confirm);
                },
            );
        },
    );

    popover.set_child(Some(&menu));
    popover
}

fn add_terminal_context_menu_header(
    menu: &gtk::Box,
    workspace: &forktty_core::Workspace,
    surface: &Surface,
) {
    let header = gtk::Box::new(gtk::Orientation::Vertical, 1);
    header.add_css_class("ft-menu-header");

    let title = gtk::Label::builder()
        .label(surface_title(surface))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("ft-menu-header-title");
    let subtitle = gtk::Label::builder()
        .label(format!(
            "{} · {}",
            workspace.name,
            compact_path(&surface.cwd)
        ))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    subtitle.add_css_class("ft-menu-header-subtitle");

    header.append(&title);
    header.append(&subtitle);
    menu.append(&header);
}

fn terminal_context_snapshot(
    state: &SocketAppState,
    surface_id: &str,
) -> Option<(forktty_core::Workspace, Surface)> {
    let model = state.model.lock().ok()?;
    let surface = model.surface(surface_id)?.clone();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == surface.workspace_id)?;
    Some((workspace, surface))
}

fn build_terminal_context_menu(
    state: &SocketAppState,
    surface_id: &str,
    terminal: &VteTerminalWidget,
    parent: &adw::ApplicationWindow,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");

    let snapshot = terminal_context_snapshot(state, surface_id);
    if let Some((workspace, surface)) = snapshot.as_ref() {
        add_terminal_context_menu_header(&menu, workspace, surface);
        add_context_menu_separator(&menu);
    }

    let terminal_for_copy = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-copy-symbolic",
        "Copy",
        false,
        move || terminal_for_copy.copy_clipboard_format(Format::Text),
    );

    let terminal_for_paste = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-paste-symbolic",
        "Paste",
        false,
        move || terminal_for_paste.paste_clipboard(),
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-dual-symbolic",
        "Split Right",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Horizontal)
            });
        },
    );

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-paged-symbolic",
        "Split Down",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Vertical)
            });
        },
    );

    add_context_menu_separator(&menu);

    if let Some((workspace, surface)) = snapshot {
        let workspace_id = workspace.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy Workspace ID",
            false,
            move || copy_to_clipboard(&workspace_id),
        );

        let surface_id = surface.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy Surface ID",
            false,
            move || copy_to_clipboard(&surface_id),
        );

        let identifiers = format!(
            "workspace_id={}\nsurface_id={}\nsocket_path={}",
            workspace.id,
            surface.id,
            state.socket_path.to_string_lossy()
        );
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy IDs",
            false,
            move || copy_to_clipboard(&identifiers),
        );

        let cwd = surface.cwd.to_string_lossy().to_string();
        add_context_menu_item(
            &menu,
            &popover,
            "folder-symbolic",
            "Copy Working Directory",
            false,
            move || copy_to_clipboard(&cwd),
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-refresh-symbolic",
        "Restart Pane",
        false,
        move || {
            restart_surface(&state_, &sid);
        },
    );

    let state_ = state.clone();
    let parent_ = parent.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Pane",
        true,
        move || {
            show_close_pane_confirmation(&parent_, &state_, &sid);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

fn install_terminal_context_menu(
    widget: &VteTerminalWidget,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widget_for_menu = widget.clone();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_menu.grab_focus();
        if let Ok(mut model) = state_for_menu.model.lock() {
            let _ = model.focus_surface(&surface_id_for_menu);
            let _ = model.mark_surface_unread(&surface_id_for_menu, false);
        }
        if let Some(popover) = current_popover_for_menu.borrow_mut().take() {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        let popover = build_terminal_context_menu(
            &state_for_menu,
            &surface_id_for_menu,
            &widget_for_menu,
            &parent_for_menu,
        );
        let current_popover_for_closed = current_popover_for_menu.clone();
        popover.connect_closed(move |popover| {
            let should_clear = current_popover_for_closed
                .borrow()
                .as_ref()
                .is_some_and(|current| current == popover);
            if should_clear {
                current_popover_for_closed.borrow_mut().take();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        popover.set_parent(&widget_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        *current_popover_for_menu.borrow_mut() = Some(popover.clone());
        popover.popup();
    });
    widget.add_controller(gesture);
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
        return missing_surface_placeholder("unknown", None, None).upcast();
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
        let done =
            (applied && attempt >= PANED_RATIO_APPLY_FRAMES) || attempt >= PANED_RATIO_MAX_FRAMES;
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

struct SurfacePlaceholderDetails {
    icon_name: &'static str,
    title: &'static str,
    description: String,
    can_restart: bool,
}

fn missing_surface_placeholder(
    surface_id: &str,
    state: Option<&SocketAppState>,
    model: Option<&Arc<Mutex<WorkspaceModel>>>,
) -> gtk::Box {
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

    let details = surface_placeholder_details(model, surface_id);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("terminal-placeholder");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let status = compact_status_page(details.icon_name, details.title, &details.description);
    body.append(&status);

    if let Some(state) = state {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("terminal-placeholder-actions");
        actions.set_halign(gtk::Align::Center);
        if details.can_restart {
            let (restart, _, _) =
                labeled_icon_button_parts("view-refresh-symbolic", "Restart Pane");
            restart.add_css_class("suggested-action");
            let state_for_restart = state.clone();
            let surface_id_for_restart = surface_id.to_string();
            restart.connect_clicked(move |_| {
                restart_surface(&state_for_restart, &surface_id_for_restart);
            });
            actions.append(&restart);
        }
        let (copy_id, _, _) = labeled_icon_button_parts("edit-copy-symbolic", "Copy Surface ID");
        let surface_id_for_copy = surface_id.to_string();
        copy_id.connect_clicked(move |_| copy_to_clipboard(&surface_id_for_copy));
        actions.append(&copy_id);
        body.append(&actions);
    }

    pane.append(&header);
    pane.append(&body);
    pane
}

fn empty_terminal_stage(
    state: Option<&SocketAppState>,
    parent: Option<&adw::ApplicationWindow>,
) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 10);
    container.add_css_class("terminal-empty-stage");
    container.set_hexpand(true);
    container.set_vexpand(true);

    let status = compact_status_page(
        "utilities-terminal-symbolic",
        "No Workspace Open",
        "Create a workspace to start a terminal session.",
    );
    container.append(&status);

    if let Some(state) = state {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("terminal-placeholder-actions");
        actions.set_halign(gtk::Align::Center);
        let (create, _, _) = labeled_icon_button_parts("tab-new-symbolic", "New Workspace");
        create.add_css_class("suggested-action");
        let state_for_create = state.clone();
        create.connect_clicked(move |_| create_plain_workspace(&state_for_create));
        actions.append(&create);
        if let Some(parent) = parent {
            let (open, _, _) = labeled_icon_button_parts("folder-open-symbolic", "Open Workspace");
            let state_for_open = state.clone();
            let parent_for_open = parent.clone();
            open.connect_clicked(move |_| open_workspace_dialog(&parent_for_open, &state_for_open));
            actions.append(&open);
        }
        container.append(&actions);

        let hints = gtk::Box::new(gtk::Orientation::Vertical, 6);
        hints.add_css_class("terminal-empty-shortcuts");
        hints.set_halign(gtk::Align::Center);
        hints.append(&shortcut_hint("Ctrl+Shift+N", "New Workspace"));
        hints.append(&shortcut_hint("Ctrl+Shift+O", "Open Workspace"));
        hints.append(&shortcut_hint("Ctrl+Shift+P", "Command Palette"));
        container.append(&hints);
    }

    container
}

fn shortcut_hint(shortcut: &str, label: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("shortcut-hint");
    let key = gtk::Label::new(Some(shortcut));
    key.add_css_class("keycap");
    key.add_css_class("monospace");
    let text = gtk::Label::builder().label(label).xalign(0.0).build();
    text.add_css_class("ft-form-hint");
    row.append(&key);
    row.append(&text);
    row
}

fn surface_placeholder_details(
    model: Option<&Arc<Mutex<WorkspaceModel>>>,
    surface_id: &str,
) -> SurfacePlaceholderDetails {
    let status = model.and_then(|model| model.lock().ok()).and_then(|model| {
        let surface = model.surface(surface_id)?;
        model
            .list_status(&surface.workspace_id)
            .into_iter()
            .find(|entry| entry.key == surface_status_key(surface_id))
    });

    if let Some(status) = status {
        if status_entry_suggests_error(&status) {
            return SurfacePlaceholderDetails {
                icon_name: "dialog-error-symbolic",
                title: "Terminal Failed to Start",
                description: format!(
                    "{}. Check the shell/settings, then restart this pane.",
                    truncate_single_line(&status.value, 180)
                ),
                can_restart: true,
            };
        }
        if status_entry_suggests_exited(&status) {
            return SurfacePlaceholderDetails {
                icon_name: "dialog-warning-symbolic",
                title: "Terminal Exited",
                description: format!(
                    "{}. Restart this pane to open a new shell in the same directory.",
                    truncate_single_line(&status.value, 180)
                ),
                can_restart: true,
            };
        }
        if status.value.to_ascii_lowercase().contains("restarting") {
            return SurfacePlaceholderDetails {
                icon_name: "view-refresh-symbolic",
                title: "Starting Terminal",
                description: "ForkTTY is starting this pane.".to_string(),
                can_restart: false,
            };
        }
    }

    SurfacePlaceholderDetails {
        icon_name: "utilities-terminal-symbolic",
        title: "Terminal Pending",
        description: format!("Surface {surface_id} has not spawned yet."),
        can_restart: true,
    }
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let (app_config, config_load_warning) = match config::load_config() {
        Ok(config) => (config, None),
        Err(err) => (
            config::AppConfig::default(),
            Some(format!("Could not load config; defaults are in use. {err}")),
        ),
    };
    apply_color_scheme(&app_config);
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
    if let Some(message) = config_load_warning.as_deref() {
        create_global_notification(&state, "Config Issue", message, NotificationKind::Error);
    }
    let ui_alive = Rc::new(Cell::new(true));

    let header = adw::HeaderBar::new();
    header.add_css_class("app-header");
    let workspace_title = gtk::Button::builder().label("").has_frame(false).build();
    workspace_title.add_css_class("flat");
    workspace_title.add_css_class("app-header-title");
    workspace_title.set_tooltip_text(Some("Switch workspace"));
    workspace_title.set_sensitive(false);
    set_accessible_button_text(&workspace_title, "No active workspace", None);
    header.set_title_widget(Some(&workspace_title));

    let command_palette = gtk::Button::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Command Palette (Ctrl+Shift+P)")
        .build();
    let notifications = gtk::Button::builder()
        .icon_name("preferences-system-notifications-symbolic")
        .tooltip_text("Notifications (Ctrl+Shift+M)")
        .build();
    let settings = gtk::Button::builder()
        .icon_name("preferences-system-symbolic")
        .tooltip_text("Settings (Ctrl+,)")
        .build();
    for (button, label, shortcut) in [
        (&command_palette, "Command Palette", Some("Ctrl+Shift+P")),
        (&notifications, "Notifications", Some("Ctrl+Shift+M")),
        (&settings, "Settings", Some("Ctrl+,")),
    ] {
        button.add_css_class("flat");
        button.add_css_class("header-action");
        set_accessible_button_text(button, label, shortcut);
    }
    refresh_notification_indicator(&notifications, &state);

    // Global app tools stay in the titlebar; workspace creation lives in the sidebar.
    let header_action_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    header_action_separator.add_css_class("header-action-separator");
    header.pack_end(&header_action_separator);
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
    set_sidebar_position_class(&sidebar_shell, &app_config.appearance.sidebar_position);

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
    set_accessible_button_text(&sidebar_add, "New Workspace", None);
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
    let status_location = gtk::Button::builder().label("").has_frame(false).build();
    status_location.add_css_class("flat");
    status_location.add_css_class("status-location");
    status_location.set_tooltip_text(Some("Switch workspace"));
    status_location.set_sensitive(false);
    let status_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_spacer.set_hexpand(true);
    let pane_status = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    pane_status.add_css_class("pane-status");
    let palette_hint = gtk::Button::builder()
        .label("Ctrl+Shift+P")
        .has_frame(false)
        .tooltip_text("Open Command Palette")
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

    let controller = Rc::new(RefCell::new(VteController::new(
        terminal_stack.borrow().clone(),
        window.clone(),
        model.clone(),
    )));
    controller.borrow_mut().attach_state(state.clone());

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
        status_location: status_location.clone(),
        pane_status: pane_status.clone(),
        last_signature: Rc::new(RefCell::new(None::<String>)),
        context_menu_open: Rc::new(Cell::new(false)),
        context_popover: Rc::new(RefCell::new(None)),
    };
    let controller_for_timer = controller.clone();
    let alive_for_terminal_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        if !alive_for_terminal_timer.get() {
            return glib::ControlFlow::Break;
        }
        // VTE/GTK on Wayland can become unstable if several terminals are
        // created and spawned in the same main-loop turn during session restore.
        // Handle one backend command per frame so restored panes realize
        // incrementally.
        if let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
        }
        controller_for_timer.borrow_mut().ensure_layout_current();
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
    install_session_autosave(&state);

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

    let settings_parent = window.clone();
    let settings_apply_for_button = settings_apply.clone();
    settings.connect_clicked(move |_| {
        show_settings_dialog(&settings_parent, settings_apply_for_button.clone());
    });

    install_actions(
        app,
        &window,
        &state,
        &sidebar_shell,
        &controller,
        settings_apply,
        quake_mode,
    );
    if quake_mode {
        install_global_quake_shortcut(&window);
    }
    let state_for_close = state.clone();
    let alive_for_close = ui_alive.clone();
    window.connect_close_request(move |_| {
        alive_for_close.set(false);
        save_session_from_state(&state_for_close);
        glib::Propagation::Proceed
    });

    window.present();
    start_socket_server(state.clone());

    let state_for_bootstrap = state.clone();
    let controller_for_bootstrap = controller.clone();
    let sidebar_ui_for_bootstrap = sidebar_ui.clone();
    glib::idle_add_local_once(move || {
        if let Err(err) = restore_or_bootstrap_workspaces(&state_for_bootstrap, cwd) {
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
    });
}

fn settings_apply_callback(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    terminal_stack: &gtk::Box,
    controller: &Rc<RefCell<VteController>>,
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
        for widget in controller.borrow().widgets.values() {
            apply_vte_appearance(widget);
        }
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

fn set_sidebar_position_class(sidebar_shell: &gtk::Box, position: &str) {
    sidebar_shell.remove_css_class("left");
    sidebar_shell.remove_css_class("right");
    if position == "right" {
        sidebar_shell.add_css_class("right");
    } else {
        sidebar_shell.add_css_class("left");
    }
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

fn apply_color_scheme(config: &config::AppConfig) {
    let scheme = match config
        .general
        .theme_source
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
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
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            model.restore_session(data);
            Ok(())
        }
        Ok(_) => {
            create_global_notification(
                state,
                "Welcome to ForkTTY",
                "Opened the current directory as the main workspace. Use Ctrl+Shift+P for commands, F9 to toggle the sidebar, and New Worktree for isolated git work.",
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

fn save_session_from_state(state: &SocketAppState) {
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

fn install_session_autosave(state: &SocketAppState) {
    let state = state.clone();
    let last_saved = Rc::new(RefCell::new(None::<String>));
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let data = match state.model.lock() {
            Ok(mut model) => {
                let _ = model.repair_session_invariants();
                model.to_session_data()
            }
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
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    if !display.backend().is_wayland() {
        eprintln!(
            "GTK display backend is {}; layer-shell quake placement requires Wayland",
            display.name()
        );
        return false;
    }

    const GTK_LAYER_SHELL_EDGE_LEFT: i32 = 0;
    const GTK_LAYER_SHELL_EDGE_RIGHT: i32 = 1;
    const GTK_LAYER_SHELL_EDGE_TOP: i32 = 2;
    const GTK_LAYER_SHELL_EDGE_BOTTOM: i32 = 3;
    const GTK_LAYER_SHELL_LAYER_TOP: i32 = 2;
    const GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND: i32 = 2;

    type IsSupported = unsafe extern "C" fn() -> i32;
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
        let is_supported = library.get::<IsSupported>(b"gtk_layer_is_supported\0").ok();
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
            Some(is_supported),
            Some(init),
            Some(set_layer),
            Some(set_anchor),
            Some(set_margin),
            Some(set_keyboard_mode),
            Some(set_namespace),
        ) = (
            is_supported,
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
        if is_supported() == 0 {
            return false;
        }

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
    controller: &Rc<RefCell<VteController>>,
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
        let settings_apply = settings_apply.clone();
        move || show_settings_dialog(&window, settings_apply.clone())
    });
    add_action(app, "shortcuts", {
        let window = window.clone();
        move || show_shortcuts_dialog(&window)
    });
    add_action(app, "restart-pane", {
        let state = state.clone();
        move || restart_active_surface(&state)
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
            controller.borrow_mut().rebuild_layout();
        }
    });
    add_action(app, "focus-next-pane", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, 1);
            controller.borrow_mut().rebuild_layout();
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
    app.set_accels_for_action("app.new-workspace", &["<Control><Shift>N"]);
    app.set_accels_for_action("app.open-workspace", &["<Control><Shift>O"]);
    app.set_accels_for_action("app.command-palette", &["<Control><Shift>P"]);
    app.set_accels_for_action("app.restart-pane", &[RESTART_PANE_ACCEL]);
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
        ui.workspace_title
            .set_tooltip_text(Some("Switch workspace"));
        set_accessible_button_text(
            &ui.workspace_title,
            &format!("Active workspace: {name}"),
            None,
        );
        ui.workspace_title.set_sensitive(true);
    } else {
        ui.workspace_title.set_label("");
        ui.workspace_title
            .set_tooltip_text(Some("No active workspace"));
        set_accessible_button_text(&ui.workspace_title, "No active workspace", None);
        ui.workspace_title.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_status_label.as_deref() {
        ui.status_location.set_label(label);
        ui.status_location
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Workspace location: {label}"
            ))]);
        ui.status_location.set_sensitive(true);
    } else {
        ui.status_location.set_label("");
        ui.status_location
            .update_property(&[gtk::accessible::Property::Label(
                "No active workspace location",
            )]);
        ui.status_location.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_pane_label.as_deref() {
        ui.pane_status.set_label(label);
        ui.pane_status.set_tooltip_text(Some(label));
        ui.pane_status.set_visible(true);
    } else {
        ui.pane_status.set_label("");
        ui.pane_status.set_tooltip_text(None);
        ui.pane_status.set_visible(false);
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
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
        empty.add_css_class("sidebar-empty");
        empty.set_halign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("folder-symbolic");
        icon.set_pixel_size(24);
        let title = gtk::Label::builder().label("No Workspaces").build();
        title.add_css_class("sidebar-empty-title");
        let body = gtk::Label::builder()
            .label("Create a workspace to start a terminal.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        body.add_css_class("sidebar-empty-body");
        let create = gtk::Button::builder()
            .label("New Workspace")
            .has_frame(true)
            .build();
        create.add_css_class("suggested-action");
        create.set_action_name(Some("app.new-workspace"));
        set_accessible_button_text(&create, "New Workspace", Some("Ctrl+Shift+N"));
        empty.append(&icon);
        empty.append(&title);
        empty.append(&body);
        empty.append(&create);
        row.set_child(Some(&empty));
        ui.sidebar.append(&row);
        return;
    }
    for row_data in snapshot.rows {
        let SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            status,
            surface_count,
        } = row_data;
        let row = gtk::ListBoxRow::new();
        row.set_selectable(true);
        row.set_activatable(true);
        row.add_css_class("workspace-row");
        let mut accessible_label = format!("Workspace {}. {}", workspace.name, meta);
        if let Some(status) = status.as_ref() {
            accessible_label.push_str(&format!(". {}", status.label));
        }
        if !summary.is_empty() {
            accessible_label.push_str(&format!(". {summary}"));
        }
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if workspace.active {
            row.add_css_class("active");
        }
        if workspace.needs_attention {
            row.add_css_class("needs-attention");
        }

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .hexpand(true)
            .build();
        card.add_css_class("workspace-card");

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .hexpand(true)
            .build();
        let name = gtk::Label::builder()
            .label(&workspace.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("workspace-name");

        let name_line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .hexpand(true)
            .build();
        name_line.add_css_class("workspace-name-line");
        name_line.append(&name);

        if let Some(status) = status.as_ref() {
            let status_badge = gtk::Label::builder()
                .label(status.label)
                .xalign(0.5)
                .build();
            status_badge.add_css_class("workspace-status-badge");
            status_badge.add_css_class(status.class_name);
            status_badge.set_tooltip_text(Some(status.label));
            name_line.append(&status_badge);
        }

        let meta_label = gtk::Label::builder()
            .label(&meta)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        meta_label.add_css_class("workspace-meta");

        text.append(&name_line);
        text.append(&meta_label);

        if !summary.is_empty() {
            let summary_label = gtk::Label::builder()
                .label(&summary)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            summary_label.add_css_class("workspace-summary");
            text.append(&summary_label);
        }

        card.append(&text);

        if surface_count > 1 {
            let count_badge = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            count_badge.add_css_class("workspace-count-badge");
            count_badge.set_valign(gtk::Align::Start);
            let count_icon = gtk::Image::from_icon_name("view-grid-symbolic");
            count_icon.add_css_class("workspace-count-icon");
            count_icon.set_tooltip_text(Some(&format!("{surface_count} panes")));
            let count_label = gtk::Label::new(Some(&surface_count.to_string()));
            count_label.add_css_class("workspace-count");
            count_label.set_tooltip_text(Some(&format!("{surface_count} panes")));
            count_badge.append(&count_icon);
            count_badge.append(&count_label);
            card.append(&count_badge);
        }

        let mut tooltip = format!("{}\n{}", workspace.name, meta);
        if let Some(status) = status.as_ref() {
            tooltip.push('\n');
            tooltip.push_str(status.label);
        }
        if !summary.is_empty() {
            tooltip.push('\n');
            tooltip.push_str(&summary);
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

        let key = gtk::EventControllerKey::new();
        let workspace_id_for_key = workspace.id.clone();
        let state_for_key = state.clone();
        let controller_for_key = controller.clone();
        let ui_for_key = ui.clone();
        let row_for_key = row.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            let should_activate = matches!(
                key,
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter | gtk::gdk::Key::space
            );
            if !should_activate {
                return glib::Propagation::Proceed;
            }
            close_sidebar_context_menu(&ui_for_key);
            ui_for_key.sidebar.select_row(Some(&row_for_key));
            select_sidebar_workspace(&state_for_key, &workspace_id_for_key, &controller_for_key);
            schedule_sidebar_refresh(
                ui_for_key.clone(),
                state_for_key.clone(),
                controller_for_key.clone(),
            );
            glib::Propagation::Stop
        });
        row.add_controller(key);

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
            active_pane_label: None,
            signature: "lock-poisoned".to_string(),
        };
    };
    let active_workspace_id = model.active_workspace_id();
    let notifications = model.list_notifications();
    let mut rows = Vec::new();
    for workspace in model.list_workspaces() {
        let statuses = model.list_status(&workspace.id);
        let progress = model.list_progress(&workspace.id);
        let logs = model.list_logs(&workspace.id);
        let latest_attention_notification = workspace
            .needs_attention
            .then(|| {
                notifications
                    .iter()
                    .rev()
                    .find(|notification| {
                        notification_targets_workspace(&model, notification, &workspace.id)
                    })
                    .cloned()
            })
            .flatten();
        let status = workspace_status_badge(
            &workspace,
            &statuses,
            &progress,
            latest_attention_notification.as_ref(),
        );
        let summary = format_workspace_activity_summary(
            &statuses,
            &progress,
            logs.first(),
            latest_attention_notification.as_ref(),
        );
        let surface_count = model.list_surfaces(Some(&workspace.id)).len();
        let meta = workspace_meta_line(&workspace);
        rows.push(SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            status,
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
    let active_pane_label = active_workspace.and_then(|workspace| {
        let leaves = collect_leaves(&workspace.pane_tree);
        let pane_count = leaves.len();
        let index = leaves
            .iter()
            .position(|surface_id| surface_id == &workspace.focused_surface_id)?;
        let surface = model.surface(&workspace.focused_surface_id);
        let title = surface.map(surface_title).unwrap_or("Terminal");
        let compact_cwd = surface
            .map(|surface| compact_path(&surface.cwd))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        let full_cwd = surface
            .map(|surface| surface.cwd.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.working_dir.to_string_lossy().to_string());
        if pane_count <= 1 {
            // With a single pane the "Pane 1/1" prefix is noise; the cwd is
            // already shown in status_location. Only surface a distinct title.
            if title == "Terminal" || title == compact_cwd.as_str() || title == full_cwd.as_str() {
                return None;
            }
            return Some(title.to_string());
        }
        Some(format!("Pane {}/{} · {}", index + 1, pane_count, title))
    });
    let mut signature = format!(
        "active={:?};status={:?};pane={:?};rows={};",
        active_workspace_id,
        active_status_label,
        active_pane_label,
        rows.len()
    );
    for row in &rows {
        signature.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?};",
            row.workspace.id,
            row.workspace.name,
            row.workspace.active,
            row.workspace.needs_attention,
            row.workspace.working_dir.to_string_lossy(),
            row.workspace.worktree_name.as_deref().unwrap_or(""),
            row.surface_count,
            row.meta,
            row.summary,
            row.status
                .as_ref()
                .map(|status| (status.label, status.class_name))
        ));
    }
    SidebarSnapshot {
        rows,
        active_workspace_name,
        active_status_label,
        active_pane_label,
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
    parts.push(compact_path(&workspace.working_dir));
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

fn focus_relative_pane(state: &SocketAppState, delta: isize) -> bool {
    let mut model = match state.model.lock() {
        Ok(model) => model,
        Err(_) => return false,
    };
    let Some(workspace) = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.active)
        .or_else(|| model.list_workspaces().into_iter().next())
    else {
        return false;
    };
    let leaves = collect_leaves(&workspace.pane_tree);
    if leaves.len() < 2 {
        return false;
    }
    let current = leaves
        .iter()
        .position(|surface_id| surface_id == &workspace.focused_surface_id)
        .unwrap_or(0);
    let len = leaves.len() as isize;
    let next = (current as isize + delta).rem_euclid(len) as usize;
    model.focus_surface(&leaves[next])
}

fn notification_targets_workspace(
    model: &WorkspaceModel,
    notification: &NotificationItem,
    workspace_id: &str,
) -> bool {
    if notification.workspace_id.as_deref() == Some(workspace_id) {
        return true;
    }
    notification
        .surface_id
        .as_deref()
        .and_then(|surface_id| model.surface(surface_id))
        .map(|surface| surface.workspace_id == workspace_id)
        .unwrap_or(false)
}

fn workspace_status_badge(
    workspace: &forktty_core::Workspace,
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_attention_notification: Option<&NotificationItem>,
) -> Option<WorkspaceStatusBadge> {
    if let Some(notification) = latest_attention_notification {
        return Some(match notification.kind {
            NotificationKind::Error => WorkspaceStatusBadge {
                label: "Error",
                class_name: "error",
            },
            NotificationKind::Prompt => WorkspaceStatusBadge {
                label: "Needs Input",
                class_name: "needs-input",
            },
            NotificationKind::Info | NotificationKind::Custom => WorkspaceStatusBadge {
                label: "Attention",
                class_name: "attention",
            },
        });
    }

    if workspace.needs_attention {
        return Some(WorkspaceStatusBadge {
            label: "Attention",
            class_name: "attention",
        });
    }

    if statuses.iter().any(status_entry_suggests_error) {
        return Some(WorkspaceStatusBadge {
            label: "Error",
            class_name: "error",
        });
    }

    if statuses.iter().any(status_entry_suggests_exited) {
        return Some(WorkspaceStatusBadge {
            label: "Exited",
            class_name: "exited",
        });
    }

    if statuses.iter().any(status_entry_suggests_running) {
        return Some(WorkspaceStatusBadge {
            label: "Running",
            class_name: "running",
        });
    }

    if !progress.is_empty() {
        return Some(WorkspaceStatusBadge {
            label: "Working",
            class_name: "working",
        });
    }

    None
}

fn surface_status_blocks_auto_spawn(statuses: &[StatusEntry], surface_id: &str) -> bool {
    let key = surface_status_key(surface_id);
    statuses.iter().any(|status| {
        status.key == key
            && (status_entry_suggests_error(status) || status_entry_suggests_exited(status))
    })
}

fn status_entry_suggests_running(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("running")
        || value.contains("working")
        || value.contains("busy")
        || color == "blue"
}

fn status_entry_suggests_error(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("error")
        || value.contains("failed")
        || value.contains("failure")
        || color == "red"
}

fn status_entry_suggests_exited(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("exited") || value.contains("stopped") || color == "yellow" || color == "warning"
}

fn format_workspace_activity_summary(
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_log: Option<&forktty_core::LogEntry>,
    latest_attention_notification: Option<&NotificationItem>,
) -> String {
    if let Some(notification) = latest_attention_notification {
        return format_notification_preview(notification);
    }
    format_metadata_summary(statuses, progress, latest_log)
}

fn format_notification_preview(notification: &NotificationItem) -> String {
    let body = notification.body.trim();
    let text = if body.is_empty() {
        notification.title.trim().to_string()
    } else {
        format!("{}: {body}", notification.title.trim())
    };
    truncate_single_line(&text, 120)
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut truncated = collapsed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
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
        surface_id: surface.id.clone(),
        workspace_id: surface.workspace_id.clone(),
        shell: state.shell.clone(),
        cwd: surface.cwd.clone(),
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        if let Ok(mut model) = state.model.lock() {
            let _ = model.close_surface(&surface.id);
        }
        eprintln!("Failed to spawn split terminal: {err}");
    } else {
        save_session_from_state(state);
    }
}

fn restart_active_surface(state: &SocketAppState) {
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
    restart_surface(state, &focused);
}

fn restart_surface(state: &SocketAppState, surface_id: &str) -> bool {
    let surface = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        model.surface(surface_id).cloned()
    };
    let Some(surface) = surface else {
        return false;
    };

    if let Ok(mut model) = state.model.lock() {
        let _ = model.set_status(
            &surface.workspace_id,
            surface_status_key(surface_id),
            "Terminal",
            "Restarting",
            Some("blue".to_string()),
        );
        let _ = model.focus_surface(surface_id);
        let _ = model.mark_surface_unread(surface_id, false);
    }

    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to restart terminal surface {surface_id}: {err}");
            return false;
        }
    }

    if let Err(err) = state.terminal.spawn(SpawnRequest {
        surface_id: surface.id.clone(),
        workspace_id: surface.workspace_id.clone(),
        shell: state.shell.clone(),
        cwd: surface.cwd.clone(),
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        record_terminal_spawn_failure(
            &state.model,
            &surface.workspace_id,
            &surface.id,
            &err.to_string(),
        );
        return false;
    }
    true
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
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        if model.surface(&focused).is_none() {
            return;
        }
    }
    if let Err(err) = state.terminal.close(&focused) {
        eprintln!("Failed to close terminal surface: {err}");
        return;
    }
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_surface(&focused);
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep focused terminal alive: {err}");
    }
    save_session_from_state(state);
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

fn show_workspace_popover<W: IsA<gtk::Widget>>(
    anchor: &W,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
) {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-workspace-popover");
    popover.set_has_arrow(false);
    popover.set_autohide(true);
    popover.set_parent(anchor);

    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    container.add_css_class("ft-workspace-popover-list");
    container.set_width_request(300);

    let (active_id, workspaces) = {
        let Ok(model) = state.model.lock() else {
            return;
        };
        (model.active_workspace_id(), model.list_workspaces())
    };

    if workspaces.is_empty() {
        let empty = gtk::Label::builder().label("No workspaces").build();
        empty.add_css_class("ft-workspace-popover-empty");
        container.append(&empty);
    } else {
        for ws in workspaces {
            let is_active = active_id.as_deref() == Some(ws.id.as_str());
            let row = gtk::Button::builder().has_frame(false).build();
            row.add_css_class("flat");
            row.add_css_class("ft-workspace-popover-row");
            if is_active {
                row.add_css_class("active");
            }

            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            inner.add_css_class("ft-workspace-popover-row-inner");

            let check = gtk::Image::from_icon_name("emblem-ok-symbolic");
            check.add_css_class("ft-workspace-popover-check");
            if !is_active {
                check.set_opacity(0.0);
            }
            inner.append(&check);

            let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
            body.set_hexpand(true);
            let name = gtk::Label::builder()
                .label(&ws.name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            name.add_css_class("ft-workspace-popover-name");
            let path = gtk::Label::builder()
                .label(compact_path(&ws.working_dir))
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            path.add_css_class("ft-workspace-popover-path");
            body.append(&name);
            body.append(&path);
            inner.append(&body);
            row.set_child(Some(&inner));

            let popover_for_row = popover.clone();
            let state_for_row = state.clone();
            let controller_for_row = controller.clone();
            let ws_id = ws.id.clone();
            row.connect_clicked(move |_| {
                popover_for_row.popdown();
                select_sidebar_workspace(&state_for_row, &ws_id, &controller_for_row);
            });
            container.append(&row);
        }
    }

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.add_css_class("ft-workspace-popover-separator");
    container.append(&separator);

    let new_btn = gtk::Button::builder().has_frame(false).build();
    new_btn.add_css_class("flat");
    new_btn.add_css_class("ft-workspace-popover-row");
    new_btn.add_css_class("ft-workspace-popover-action");
    let new_inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    new_inner.add_css_class("ft-workspace-popover-row-inner");
    let new_icon = gtk::Image::from_icon_name("tab-new-symbolic");
    new_icon.add_css_class("ft-workspace-popover-action-icon");
    new_inner.append(&new_icon);
    let new_label = gtk::Label::builder()
        .label("New Workspace")
        .xalign(0.0)
        .hexpand(true)
        .build();
    new_inner.append(&new_label);
    new_btn.set_child(Some(&new_inner));
    let popover_for_new = popover.clone();
    let state_for_new = state.clone();
    new_btn.connect_clicked(move |_| {
        popover_for_new.popdown();
        create_plain_workspace(&state_for_new);
    });
    container.append(&new_btn);

    popover.set_child(Some(&container));

    let popover_for_cleanup = popover.clone();
    popover.connect_closed(move |_| {
        popover_for_cleanup.unparent();
    });

    popover.popup();
}

fn show_command_palette_with_controller(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<VteController>>>,
) {
    show_command_palette_with_query(parent, state, "", controller);
}

fn show_shortcuts_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("Keyboard Shortcuts")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(440)
        .build();
    dialog.add_css_class("ft-dialog");
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
        .label("Common workspace, pane, and app commands.")
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
            ("Split Right", "Ctrl+Shift+H"),
            ("Split Down", SPLIT_VERTICAL_SHORTCUT),
            ("Restart Pane", RESTART_PANE_SHORTCUT),
            ("Close Pane", "Ctrl+Shift+W"),
            ("Focus Previous Pane", "Ctrl+Alt+Left"),
            ("Focus Next Pane", "Ctrl+Alt+Right"),
            ("Maximize Pane", "Ctrl+Shift+Enter"),
        ],
    );
    append_shortcut_group(
        &content,
        "Workspaces",
        &[
            ("New Workspace", "Ctrl+Shift+N"),
            ("Open Workspace", "Ctrl+Shift+O"),
            ("Command Palette", "Ctrl+Shift+P"),
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
            ("Context Menu", "Right Click"),
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

fn append_shortcut_group(container: &gtk::Box, title: &str, shortcuts: &[(&str, &str)]) {
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

fn show_command_palette_with_query(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    initial_query: &str,
    controller: Option<Rc<RefCell<VteController>>>,
) {
    let dialog = gtk::Window::builder()
        .title("Command Palette")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(360)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

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
    command!("New Workspace", Some("Ctrl+Shift+N"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_plain_workspace(&state);
            dialog.close();
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
    command!("Keyboard Shortcuts", None, {
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
    command!("Close Pane", Some("Ctrl+Shift+W"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            close_active_surface(&state);
            dialog.close();
        }
    });
    command!("Focus Previous Pane", Some("Ctrl+Alt+Left"), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, -1);
            if let Some(controller) = &controller {
                controller.borrow_mut().rebuild_layout();
            }
            dialog.close();
        }
    });
    command!("Focus Next Pane", Some("Ctrl+Alt+Right"), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, 1);
            if let Some(controller) = &controller {
                controller.borrow_mut().rebuild_layout();
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
            let state_confirm = state.clone();
            show_destructive_confirmation(
                &parent,
                "Close Workspace?",
                "Close the active workspace and all panes inside it. Running terminal processes in this workspace will be closed.",
                "Close Workspace",
                move || close_active_workspace(&state_confirm),
            );
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

fn command_matches(label: &str, query: &str) -> bool {
    if query.is_empty() || label.contains(query) {
        return true;
    }
    query
        .split_whitespace()
        .all(|token| is_subsequence(token, label))
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
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
        .default_width(520)
        .default_height(360)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Worktree").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Choose one worktree action, then enter the branch or worktree name.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let context_text = state
        .model
        .lock()
        .ok()
        .and_then(|model| {
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.active)
                .or_else(|| model.list_workspaces().into_iter().next())
                .map(|workspace| {
                    format!(
                        "Base: {} · {}",
                        workspace.name,
                        compact_path(&workspace.working_dir)
                    )
                })
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| format!("Base: {}", compact_path(&path)))
                .unwrap_or_else(|_| "Base: current directory".to_string())
        });
    let context = gtk::Label::builder()
        .label(context_text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    context.add_css_class("worktree-context");

    let mode = Rc::new(Cell::new(WorktreeDialogMode::Create));
    let mode_selector = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode_selector.add_css_class("worktree-mode-selector");
    mode_selector.add_css_class("linked");
    let create_mode = worktree_mode_button("Create", true);
    let attach_mode = worktree_mode_button("Attach", false);
    let merge_mode = worktree_mode_button("Merge", false);
    let remove_mode = worktree_mode_button("Remove", false);
    attach_mode.set_group(Some(&create_mode));
    merge_mode.set_group(Some(&create_mode));
    remove_mode.set_group(Some(&create_mode));
    mode_selector.append(&create_mode);
    mode_selector.append(&attach_mode);
    mode_selector.append(&merge_mode);
    mode_selector.append(&remove_mode);

    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name (e.g. feature/login)")
        .hexpand(true)
        .build();
    entry.add_css_class("monospace");
    entry.update_property(&[gtk::accessible::Property::Label("Branch or worktree name")]);
    entry.set_tooltip_text(Some(
        "Branch name for Create/Attach, or existing worktree name for Remove/Merge",
    ));
    let hint = gtk::Label::builder()
        .label(WorktreeDialogMode::Create.hint())
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("ft-form-hint");

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    let (primary, primary_icon, primary_label) =
        labeled_icon_button_parts("list-add-symbolic", "Create Worktree");
    primary.add_css_class("suggested-action");
    primary.set_sensitive(false);
    let cancel = gtk::Button::with_label("Cancel");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    body.add_css_class("ft-dialog-body");
    body.append(&context);
    body.append(&mode_selector);
    body.append(&entry);
    body.append(&hint);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&primary);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);
    content.append(&footer);

    dialog.set_default_widget(Some(&primary));
    entry.set_activates_default(true);
    install_escape_close(&dialog);

    let controls = WorktreeDialogControls {
        entry: entry.clone(),
        hint: hint.clone(),
        status: status.clone(),
        primary: primary.clone(),
        primary_icon: primary_icon.clone(),
        primary_label: primary_label.clone(),
    };
    let refresh = Rc::new({
        let mode = mode.clone();
        let controls = controls.clone();
        move |validate: bool| {
            refresh_worktree_dialog(mode.get(), &controls, validate);
        }
    });
    refresh(false);

    entry.connect_changed({
        let refresh = refresh.clone();
        move |_| refresh(true)
    });

    for (button, next_mode) in [
        (create_mode.clone(), WorktreeDialogMode::Create),
        (attach_mode.clone(), WorktreeDialogMode::Attach),
        (merge_mode.clone(), WorktreeDialogMode::Merge),
        (remove_mode.clone(), WorktreeDialogMode::Remove),
    ] {
        let mode = mode.clone();
        let refresh = refresh.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                mode.set(next_mode);
                refresh(false);
            }
        });
    }

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let state_for_action = state.clone();
    let status_for_action = status.clone();
    let entry_for_action = entry.clone();
    let dialog_for_action = dialog.clone();
    let mode_for_action = mode.clone();
    primary.connect_clicked(move |_| {
        let name = entry_for_action.text().trim().to_string();
        if let Err(err) = validate_worktree_name_for_gtk(&name) {
            set_status_message(&status_for_action, &err, StatusKind::Error);
            return;
        }

        match mode_for_action.get() {
            WorktreeDialogMode::Create => {
                match open_worktree_from_gtk(&state_for_action, &name, WorktreeAction::Create) {
                    Ok(()) => dialog_for_action.close(),
                    Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
                }
            }
            WorktreeDialogMode::Attach => {
                match open_worktree_from_gtk(&state_for_action, &name, WorktreeAction::Attach) {
                    Ok(()) => dialog_for_action.close(),
                    Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
                }
            }
            WorktreeDialogMode::Merge => match merge_worktree_from_gtk(&state_for_action, &name) {
                Ok(message) => set_status_message(&status_for_action, &message, StatusKind::Success),
                Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
            },
            WorktreeDialogMode::Remove => {
                let state_confirm = state_for_action.clone();
                let status_confirm = status_for_action.clone();
                let dialog_confirm = dialog_for_action.clone();
                show_destructive_confirmation(
                    &dialog_for_action,
                    "Remove Worktree?",
                    &format!(
                        "Remove worktree '{name}' and close its ForkTTY workspace. This cannot be undone from ForkTTY."
                    ),
                    "Remove Worktree",
                    move || match remove_worktree_from_gtk(&state_confirm, &name) {
                        Ok(()) => dialog_confirm.close(),
                        Err(err) => set_status_message(&status_confirm, &err, StatusKind::Error),
                    },
                );
            }
        }
    });

    dialog.set_child(Some(&content));
    dialog.present();
    entry.grab_focus();
}

#[derive(Clone)]
struct WorktreeDialogControls {
    entry: gtk::Entry,
    hint: gtk::Label,
    status: gtk::Label,
    primary: gtk::Button,
    primary_icon: gtk::Image,
    primary_label: gtk::Label,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorktreeDialogMode {
    Create,
    Attach,
    Merge,
    Remove,
}

impl WorktreeDialogMode {
    fn action_label(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create Worktree",
            WorktreeDialogMode::Attach => "Attach Worktree",
            WorktreeDialogMode::Merge => "Merge Worktree",
            WorktreeDialogMode::Remove => "Remove Worktree",
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "list-add-symbolic",
            WorktreeDialogMode::Attach => "folder-open-symbolic",
            WorktreeDialogMode::Merge => "view-converge-symbolic",
            WorktreeDialogMode::Remove => "edit-delete-symbolic",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => {
                "Creates a new git worktree from the active workspace repository."
            }
            WorktreeDialogMode::Attach => {
                "Attaches an existing branch or worktree and opens it as a workspace."
            }
            WorktreeDialogMode::Merge => {
                "Merges the named worktree branch back into the repository checkout."
            }
            WorktreeDialogMode::Remove => {
                "Removes the named worktree and closes the matching ForkTTY workspace."
            }
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create | WorktreeDialogMode::Attach => {
                "Branch name (e.g. feature/login)"
            }
            WorktreeDialogMode::Merge | WorktreeDialogMode::Remove => {
                "Existing worktree or branch name"
            }
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create a new worktree branch",
            WorktreeDialogMode::Attach => "Attach an existing worktree branch",
            WorktreeDialogMode::Merge => "Merge the named worktree branch",
            WorktreeDialogMode::Remove => "Remove the named worktree",
        }
    }

    fn destructive(self) -> bool {
        self == WorktreeDialogMode::Remove
    }
}

fn worktree_mode_button(label: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::with_label(label);
    button.add_css_class("worktree-mode-button");
    button.set_hexpand(true);
    button.set_active(active);
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn refresh_worktree_dialog(
    mode: WorktreeDialogMode,
    controls: &WorktreeDialogControls,
    validate: bool,
) {
    controls
        .entry
        .set_placeholder_text(Some(mode.placeholder()));
    controls.entry.set_tooltip_text(Some(mode.tooltip()));
    controls.hint.set_label(mode.hint());
    controls.primary_icon.set_icon_name(Some(mode.icon_name()));
    controls.primary_label.set_text(mode.action_label());
    controls.primary.set_tooltip_text(Some(mode.tooltip()));
    set_accessible_button_text(&controls.primary, mode.action_label(), None);
    controls.primary.remove_css_class("suggested-action");
    controls.primary.remove_css_class("destructive-action");
    if mode.destructive() {
        controls.primary.add_css_class("destructive-action");
    } else {
        controls.primary.add_css_class("suggested-action");
    }

    let name = controls.entry.text();
    let trimmed = name.trim();
    let valid = if trimmed.is_empty() {
        false
    } else {
        match validate_worktree_name_for_gtk(trimmed) {
            Ok(_) => true,
            Err(err) => {
                if validate {
                    set_status_message(&controls.status, &err, StatusKind::Error);
                }
                false
            }
        }
    };
    if valid || trimmed.is_empty() || !validate {
        clear_status_message(&controls.status);
    }
    controls.primary.set_sensitive(valid);
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
    let name = validate_worktree_name_for_gtk(name)?;
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

    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_worktree_workspace(
                &info.branch,
                PathBuf::from(&info.path),
                &info.branch,
                &info.worktree_name,
            ),
            previous_active_id,
        )
    };
    if let Err(err) = state
        .terminal
        .spawn(SpawnRequest {
            surface_id: workspace.focused_surface_id.clone(),
            workspace_id: workspace.id.clone(),
            shell: state.shell.clone(),
            cwd: workspace.working_dir.clone(),
            socket_path: state.socket_path.clone(),
            extra_env: Vec::new(),
        })
        .map_err(|err| err.to_string())
    {
        rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id)?;
        return Err(err);
    }
    save_session_from_state(state);
    Ok(())
}

fn remove_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<(), String> {
    let name = validate_worktree_name_for_gtk(name)?;
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
    save_session_from_state(state);
    Ok(())
}

fn merge_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<String, String> {
    let name = validate_worktree_name_for_gtk(name)?;
    let cwd = active_workspace_cwd_string(state)?;
    let result = worktree::merge(&cwd, name).map_err(|err| err.to_string())?;
    Ok(if result.trim().is_empty() {
        "Merged".to_string()
    } else {
        result
    })
}

fn validate_worktree_name_for_gtk(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Branch or worktree name is required".to_string());
    }
    if trimmed.len() > 255 {
        return Err("Branch or worktree name must be 255 bytes or fewer".to_string());
    }
    if trimmed.contains('\0') || trimmed.contains('\\') {
        return Err("Branch or worktree name contains unsupported characters".to_string());
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("Branch or worktree name contains an unsafe path segment".to_string());
    }
    Ok(trimmed)
}

fn rollback_workspace_creation_gtk(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_workspace(WorkspaceSelector::Id(workspace_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(())
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

fn notification_target_exists(state: &SocketAppState, notification: &NotificationItem) -> bool {
    let Ok(model) = state.model.lock() else {
        return false;
    };
    if notification
        .workspace_id
        .as_deref()
        .is_some_and(|workspace_id| {
            model
                .list_workspaces()
                .iter()
                .any(|workspace| workspace.id == workspace_id)
        })
    {
        return true;
    }
    notification
        .surface_id
        .as_deref()
        .is_some_and(|surface_id| model.surface(surface_id).is_some())
}

fn notification_target_label(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> Option<String> {
    let model = state.model.lock().ok()?;
    if let Some(surface_id) = notification.surface_id.as_deref() {
        if let Some(surface) = model.surface(surface_id) {
            let workspace_name = model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == surface.workspace_id)
                .map(|workspace| workspace.name)
                .unwrap_or_else(|| surface.workspace_id.clone());
            return Some(format!(
                "{} · {}",
                workspace_name,
                compact_path(&surface.cwd)
            ));
        }
    }
    notification
        .workspace_id
        .as_deref()
        .and_then(|workspace_id| {
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| {
                    format!(
                        "{} · {}",
                        workspace.name,
                        compact_path(&workspace.working_dir)
                    )
                })
        })
}

fn open_notification_target(
    state: &SocketAppState,
    controller: Option<&Rc<RefCell<VteController>>>,
    notification: &NotificationItem,
) -> bool {
    let (workspace_id, surface_id) = {
        let Ok(model) = state.model.lock() else {
            return false;
        };
        let surface_id = notification.surface_id.clone();
        let workspace_id = notification.workspace_id.clone().or_else(|| {
            surface_id
                .as_deref()
                .and_then(|surface_id| model.surface(surface_id))
                .map(|surface| surface.workspace_id.clone())
        });
        (workspace_id, surface_id)
    };
    let Some(workspace_id) = workspace_id else {
        return false;
    };

    {
        let Ok(mut model) = state.model.lock() else {
            return false;
        };
        let _ = model.select_workspace(WorkspaceSelector::Id(&workspace_id));
        if let Some(surface_id) = surface_id.as_deref() {
            let _ = model.focus_surface(surface_id);
            let _ = model.mark_surface_unread(surface_id, false);
        }
    }

    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to open notification target: {err}");
    }
    if let Some(controller) = controller {
        controller.borrow_mut().rebuild_layout();
    }
    true
}

fn show_notification_panel(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<VteController>>>,
) {
    let dialog = gtk::Window::builder()
        .title("Notifications")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(420)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

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
    let latest_openable = notifications
        .iter()
        .rev()
        .find(|notification| notification_target_exists(state, notification))
        .cloned();

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
        for notification in notifications.iter().rev() {
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
            if notification_target_exists(state, notification) {
                let open = gtk::Button::with_label("Open");
                open.add_css_class("flat");
                open.add_css_class("notification-open");
                open.set_tooltip_text(Some("Open the workspace for this notification"));
                let state_for_open = state.clone();
                let controller_for_open = controller.clone();
                let notification_for_open = notification.clone();
                let dialog_for_open = dialog.clone();
                open.connect_clicked(move |_| {
                    if open_notification_target(
                        &state_for_open,
                        controller_for_open.as_ref(),
                        &notification_for_open,
                    ) {
                        dialog_for_open.close();
                    }
                });
                top.append(&open);
            }

            let body_label = gtk::Label::builder()
                .label(&notification.body)
                .xalign(0.0)
                .wrap(true)
                .build();
            body_label.add_css_class("notification-body");

            card.append(&top);
            card.append(&body_label);
            if let Some(target) = notification_target_label(state, notification) {
                let target_label = gtk::Label::builder()
                    .label(target)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                target_label.add_css_class("notification-target");
                card.append(&target_label);
            }
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
    let jump = gtk::Button::with_label("Open Latest");
    jump.set_sensitive(latest_openable.is_some());
    jump.set_tooltip_text(Some("Open the latest notification with a workspace target"));
    let clear = gtk::Button::with_label("Clear All");
    clear.set_sensitive(has_notifications);
    clear.add_css_class("destructive-action");
    clear.set_tooltip_text(Some("Clear pending notifications"));

    footer.append(&spacer);
    footer.append(&close_button);
    footer.append(&jump);
    footer.append(&clear);

    let dialog_for_close = dialog.clone();
    close_button.connect_clicked(move |_| dialog_for_close.close());

    if let Some(notification) = latest_openable {
        let state_for_jump = state.clone();
        let controller_for_jump = controller.clone();
        let dialog_for_jump = dialog.clone();
        jump.connect_clicked(move |_| {
            if open_notification_target(
                &state_for_jump,
                controller_for_jump.as_ref(),
                &notification,
            ) {
                dialog_for_jump.close();
            }
        });
    }

    let state_for_clear = state.clone();
    let body_for_clear = body.clone();
    let subtitle_for_clear = subtitle.clone();
    let clear_for_clear = clear.clone();
    let jump_for_clear = jump.clone();
    clear.connect_clicked(move |_| {
        if let Ok(mut model) = state_for_clear.model.lock() {
            model.clear_notifications();
        }
        while let Some(child) = body_for_clear.first_child() {
            body_for_clear.remove(&child);
        }
        let empty = compact_status_page(
            "preferences-system-notifications-symbolic",
            "All Clear",
            "Prompts and alerts will appear here.",
        );
        body_for_clear.append(&empty);
        subtitle_for_clear.set_label("No pending notifications");
        clear_for_clear.set_sensitive(false);
        jump_for_clear.set_sensitive(false);
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
        .default_width(640)
        .default_height(620)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);
    let loaded = config::load_config().unwrap_or_default();

    let shell_entry = gtk::Entry::builder()
        .text(&loaded.general.shell)
        .hexpand(true)
        .build();
    let font_family = font_family_combo(parent, &loaded.appearance.font_family);
    font_family.set_tooltip_text(Some("Terminal font family"));
    let font_size = gtk::SpinButton::with_range(8.0, 64.0, 1.0);
    font_size.set_value(f64::from(loaded.appearance.font_size));
    font_size.set_numeric(true);
    font_size.set_width_chars(4);
    let notification_command = gtk::Entry::builder()
        .text(&loaded.general.notification_command)
        .placeholder_text("/usr/bin/notify-send ForkTTY")
        .hexpand(true)
        .build();
    let theme_source = combo_with_ids(
        &[("auto", "System"), ("light", "Light"), ("dark", "Dark")],
        &loaded.general.theme_source,
    );
    theme_source.set_tooltip_text(Some("Application color scheme"));
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
    let desktop_notifications = gtk::Switch::builder()
        .active(loaded.notifications.desktop)
        .tooltip_text("Forward ForkTTY notifications to the system notification daemon")
        .valign(gtk::Align::Center)
        .build();
    let notification_sound = gtk::Switch::builder()
        .active(loaded.notifications.sound)
        .tooltip_text("Play the default system alert sound when a notification fires")
        .valign(gtk::Align::Center)
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
        .label("Saved preferences update the user config file.")
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    body.add_css_class("ft-dialog-body");

    fn settings_group(title: &str) -> gtk::Box {
        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let header = gtk::Label::builder().label(title).xalign(0.0).build();
        header.add_css_class("ft-section-title");
        let group = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        group.add_css_class("settings-group");
        outer.append(&header);
        outer.append(&group);
        outer
    }

    fn settings_group_body(group: &gtk::Box) -> gtk::Box {
        group
            .last_child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .expect("settings group body")
    }

    fn settings_row<W>(
        label_text: &str,
        description: &str,
        widget: &W,
        control_width: i32,
        expand_control: bool,
    ) -> gtk::Box
    where
        W: IsA<gtk::Accessible> + IsA<gtk::Widget>,
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("settings-row");
        row.set_valign(gtk::Align::Center);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        let label = gtk::Label::builder()
            .label(label_text)
            .xalign(0.0)
            .wrap(true)
            .build();
        label.add_css_class("ft-form-label");
        let hint = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .wrap(true)
            .build();
        hint.add_css_class("ft-form-hint");
        let accessible_label = label.upcast_ref::<gtk::Accessible>();
        widget.update_relation(&[gtk::accessible::Relation::LabelledBy(&[accessible_label])]);
        text.append(&label);
        text.append(&hint);
        row.append(&text);
        widget.set_hexpand(expand_control);
        if control_width > 0 {
            widget.set_width_request(control_width);
        }
        row.append(widget);
        row
    }

    let terminal_group = settings_group("Terminal");
    let terminal_rows = settings_group_body(&terminal_group);
    terminal_rows.append(&settings_row(
        "Shell",
        "Used for new terminal sessions. Applies after restart.",
        &shell_entry,
        320,
        true,
    ));
    terminal_rows.append(&settings_row(
        "Font family",
        "Applied to open VTE terminals when saved.",
        &font_family,
        320,
        true,
    ));
    terminal_rows.append(&settings_row(
        "Font size",
        "Terminal text size, from 8 to 64 pt.",
        &font_size,
        92,
        false,
    ));
    body.append(&terminal_group);

    let workspace_group = settings_group("Workspaces");
    let workspace_rows = settings_group_body(&workspace_group);
    workspace_rows.append(&settings_row(
        "Worktree layout",
        "Where new worktrees are placed relative to the repository root.",
        &worktree_layout,
        240,
        false,
    ));
    body.append(&workspace_group);

    let appearance_group = settings_group("Appearance");
    let appearance_rows = settings_group_body(&appearance_group);
    appearance_rows.append(&settings_row(
        "Theme",
        "Use the system preference or force a light/dark app theme.",
        &theme_source,
        240,
        false,
    ));
    appearance_rows.append(&settings_row(
        "Window mode",
        "Quake mode uses a borderless drop-down window after restart.",
        &window_mode,
        240,
        false,
    ));
    appearance_rows.append(&settings_row(
        "Sidebar position",
        "Side of the main window used for workspaces.",
        &sidebar_position,
        240,
        false,
    ));
    body.append(&appearance_group);

    let notification_group = settings_group("Notifications");
    let notification_rows = settings_group_body(&notification_group);
    notification_rows.append(&settings_row(
        "Custom command",
        "Optional absolute command to run when a notification fires.",
        &notification_command,
        320,
        true,
    ));
    notification_rows.append(&settings_row(
        "Desktop notifications",
        "Forward alerts to the system notification daemon.",
        &desktop_notifications,
        0,
        false,
    ));
    notification_rows.append(&settings_row(
        "Alert sound",
        "Play the default system alert sound for alerts.",
        &notification_sound,
        0,
        false,
    ));
    body.append(&notification_group);

    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let reset = gtk::Button::with_label("Reset to Defaults");
    reset.add_css_class("flat");
    let cancel = gtk::Button::with_label("Close");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    save.set_sensitive(false);
    footer.append(&reset);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&save);

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let mark_dirty = Rc::new({
        let save = save.clone();
        let status = status.clone();
        move || {
            save.set_sensitive(true);
            status.set_visible(false);
        }
    });
    shell_entry.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    font_family.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    font_size.connect_value_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    notification_command.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    theme_source.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    worktree_layout.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    window_mode.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    sidebar_position.connect_changed({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    desktop_notifications.connect_active_notify({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });
    notification_sound.connect_active_notify({
        let mark_dirty = mark_dirty.clone();
        move |_| mark_dirty()
    });

    {
        let shell_entry = shell_entry.clone();
        let font_family = font_family.clone();
        let font_size = font_size.clone();
        let notification_command = notification_command.clone();
        let theme_source = theme_source.clone();
        let worktree_layout = worktree_layout.clone();
        let window_mode = window_mode.clone();
        let sidebar_position = sidebar_position.clone();
        let desktop_notifications = desktop_notifications.clone();
        let notification_sound = notification_sound.clone();
        let status = status.clone();
        let save = save.clone();
        let on_apply = on_apply.clone();
        reset.connect_clicked(move |_| {
            let defaults = config::AppConfig::default();
            match config::save_config(&defaults) {
                Ok(()) => {
                    shell_entry.set_text(&defaults.general.shell);
                    let _ = font_family.set_active_id(Some(DEFAULT_FONT_FAMILY_ID));
                    font_size.set_value(f64::from(defaults.appearance.font_size));
                    notification_command.set_text(&defaults.general.notification_command);
                    let _ = theme_source.set_active_id(Some(&defaults.general.theme_source));
                    let _ = worktree_layout.set_active_id(Some(&defaults.general.worktree_layout));
                    let _ = window_mode.set_active_id(Some(&defaults.appearance.window_mode));
                    let _ =
                        sidebar_position.set_active_id(Some(&defaults.appearance.sidebar_position));
                    desktop_notifications.set_active(defaults.notifications.desktop);
                    notification_sound.set_active(defaults.notifications.sound);
                    on_apply(&defaults);
                    save.set_sensitive(false);
                    set_status_message(&status, "Defaults restored.", StatusKind::Success);
                }
                Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
            }
        });
    }

    let status_for_save = status.clone();
    let save_for_save = save.clone();
    let shell_entry_for_focus = shell_entry.clone();
    let initial_shell = loaded.general.shell.clone();
    let initial_window_mode = loaded.appearance.window_mode.clone();
    save.connect_clicked(move |_| {
        let mut next = config::load_config().unwrap_or_default();
        next.general.shell = shell_entry.text().to_string();
        if let Some(family) = font_family.active_id() {
            let family = family.to_string();
            next.appearance.font_family = match family.as_str() {
                DEFAULT_FONT_FAMILY_ID => String::new(),
                SYSTEM_MONOSPACE_FONT_FAMILY_ID => "monospace".to_string(),
                _ => family,
            };
        }
        next.appearance.font_size = font_size.value() as u16;
        next.general.notification_command = notification_command.text().to_string();
        if let Some(theme) = theme_source.active_id() {
            next.general.theme_source = theme.to_string();
        }
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
                save_for_save.set_sensitive(false);
                let message = if next.general.shell != initial_shell
                    || next.appearance.window_mode != initial_window_mode
                {
                    "Saved. Font, layout and notifications are applied now; shell/window mode apply after restart."
                } else {
                    "Saved. Changes applied."
                };
                set_status_message(&status_for_save, message, StatusKind::Success);
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
    shell_entry_for_focus.grab_focus();
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

fn font_family_combo(_parent: &impl IsA<gtk::Widget>, active_family: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    let active_family = active_family.trim();
    let all_names = installed_font_families();
    let default_family = default_terminal_font_family(&all_names);
    combo.append(
        Some(DEFAULT_FONT_FAMILY_ID),
        &format!("Default terminal font ({default_family})"),
    );
    let system_monospace =
        resolved_system_monospace_family().unwrap_or_else(|| "monospace".to_string());
    combo.append(
        Some(SYSTEM_MONOSPACE_FONT_FAMILY_ID),
        &format!("System monospace ({system_monospace})"),
    );

    let mut names = installed_monospace_font_families();
    if names.is_empty() {
        names = all_names;
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

    let mut has_active =
        active_family.is_empty() || active_family.eq_ignore_ascii_case("monospace");
    for name in &names {
        if name == active_family {
            has_active = true;
        }
        combo.append(Some(name), name);
    }
    if !has_active {
        combo.append(Some(active_family), &format!("{active_family} (saved)"));
    }

    let active_id = if active_family.is_empty() {
        DEFAULT_FONT_FAMILY_ID
    } else if active_family.eq_ignore_ascii_case("monospace") {
        SYSTEM_MONOSPACE_FONT_FAMILY_ID
    } else {
        active_family
    };
    if !combo.set_active_id(Some(active_id)) {
        combo.set_active(Some(0));
    }
    combo
}

fn open_workspace_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::FileChooserNative::new(
        Some("Open Workspace"),
        Some(parent),
        gtk::FileChooserAction::SelectFolder,
        Some("Open"),
        Some("Cancel"),
    );
    let state = state.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            match dialog.file().and_then(|file| file.path()) {
                Some(path) => {
                    if let Err(err) = open_workspace_from_path(&state, path) {
                        eprintln!("Failed to open workspace: {err}");
                        create_global_notification(
                            &state,
                            "Open Workspace Failed",
                            &err,
                            NotificationKind::Error,
                        );
                    }
                }
                None => create_global_notification(
                    &state,
                    "Open Workspace Failed",
                    "The selected folder does not map to a local path.",
                    NotificationKind::Error,
                ),
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn open_workspace_from_path(state: &SocketAppState, path: PathBuf) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", path.display()));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("workspace")
        .to_string();
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (model.create_workspace(name, path), previous_active_id)
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest {
        surface_id: workspace.focused_surface_id.clone(),
        workspace_id: workspace.id.clone(),
        shell: state.shell.clone(),
        cwd: workspace.working_dir.clone(),
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id)?;
        return Err(err.to_string());
    }
    save_session_from_state(state);
    Ok(())
}

fn create_plain_workspace(state: &SocketAppState) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let (workspace, previous_active_id) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let count = model.list_workspaces().len() + 1;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_workspace(format!("workspace-{count}"), cwd),
            previous_active_id,
        )
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest {
        surface_id: workspace.focused_surface_id.clone(),
        workspace_id: workspace.id.clone(),
        shell: state.shell.clone(),
        cwd: workspace.working_dir.clone(),
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        let _ = rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id);
        eprintln!("Failed to create workspace terminal: {err}");
    } else {
        save_session_from_state(state);
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
    save_session_from_state(state);
}

fn create_global_notification(
    state: &SocketAppState,
    title: &str,
    body: &str,
    kind: NotificationKind,
) {
    if let Ok(mut model) = state.model.lock() {
        let notification = model.create_notification(title, body, kind, None, None);
        dispatch_notification_with_loaded_config(&notification);
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
            create_global_notification(
                &state,
                "Automation Unavailable",
                &format!(
                    "Could not bind the ForkTTY socket at {}. Socket automation is disabled. {err}",
                    state.socket_path.display()
                ),
                NotificationKind::Error,
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
                create_global_notification(
                    &state,
                    "Automation Unavailable",
                    &format!("Could not start the socket runtime. {err}"),
                    NotificationKind::Error,
                );
                return;
            }
        };
        let state_for_error = state.clone();
        if let Err(err) = runtime.block_on(serve(listener, state)) {
            eprintln!("ForkTTY socket server stopped: {err}");
            create_global_notification(
                &state_for_error,
                "Automation Stopped",
                &format!("The ForkTTY socket server stopped. {err}"),
                NotificationKind::Error,
            );
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
    fn detects_exited_terminal_status_for_sidebar_badge() {
        let status = StatusEntry {
            key: surface_status_key("surface-1"),
            label: "Terminal".to_string(),
            value: "Exited (0)".to_string(),
            color: Some("yellow".to_string()),
        };

        assert!(status_entry_suggests_exited(&status));
        assert!(!status_entry_suggests_error(&status));
    }

    #[test]
    fn blocks_auto_spawn_after_terminal_failure_until_restart() {
        let failed = StatusEntry {
            key: surface_status_key("surface-1"),
            label: "Terminal".to_string(),
            value: "Spawn failed: /bin/missing".to_string(),
            color: Some("red".to_string()),
        };
        let restarting = StatusEntry {
            key: surface_status_key("surface-1"),
            label: "Terminal".to_string(),
            value: "Restarting".to_string(),
            color: Some("blue".to_string()),
        };

        assert!(surface_status_blocks_auto_spawn(
            std::slice::from_ref(&failed),
            "surface-1"
        ));
        assert!(!surface_status_blocks_auto_spawn(
            &[restarting],
            "surface-1"
        ));
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
    fn default_terminal_font_prefers_installed_nerd_font() {
        let families = vec![
            "Noto Sans Mono".to_string(),
            "JetBrainsMono Nerd Font Mono".to_string(),
        ];

        assert_eq!(
            default_terminal_font_family(&families),
            "JetBrainsMono Nerd Font Mono"
        );
    }

    #[test]
    fn parses_fontconfig_family_lists() {
        let families = parse_fontconfig_families(
            "JetBrainsMono Nerd Font Mono,JetBrainsMono NFM\nNoto Sans Mono\n\n",
        );

        assert!(families.contains(&"JetBrainsMono Nerd Font Mono".to_string()));
        assert!(families.contains(&"JetBrainsMono NFM".to_string()));
        assert!(families.contains(&"Noto Sans Mono".to_string()));
    }

    #[test]
    fn first_fontconfig_family_preserves_match_order() {
        assert_eq!(
            first_fontconfig_family("Noto Sans Mono,Noto Sans Mono Regular\n"),
            Some("Noto Sans Mono".to_string())
        );
    }

    #[test]
    fn validates_worktree_names_for_gtk_actions() {
        assert_eq!(
            validate_worktree_name_for_gtk(" feature/login ").unwrap(),
            "feature/login"
        );
        assert!(validate_worktree_name_for_gtk("../escape").is_err());
        assert!(validate_worktree_name_for_gtk("feature//empty").is_err());
        assert!(validate_worktree_name_for_gtk("feature\\windows").is_err());
        assert!(validate_worktree_name_for_gtk("").is_err());
    }
}
