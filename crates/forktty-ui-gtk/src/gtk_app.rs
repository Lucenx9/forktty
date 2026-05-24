use adw::prelude::*;
use forktty_core::{
    command_safety::is_executable_file, config, dispatch_notification, session,
    validate_worktree_name, worktree, LogLevel, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, Surface, WorkspaceModel, WorkspaceSelector,
    WorktreeNameError,
};
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, default_socket_path, serve, SocketAppState,
};
use forktty_terminal::vte::{
    send_text as vte_send_text, spawn_vte_terminal_with_callback, CursorBlinkMode, CursorShape,
    Format, TerminalExt, TerminalExtManual, VteTerminalWidget,
};
use forktty_terminal::{SpawnRequest, TerminalBackend, TerminalError, TerminalSurfaceState};
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::translate::ToGlibPtr;
use gtk::glib::types::StaticType;
use gtk4 as gtk;
use libloading::Library;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.forktty.ForkTTY";
const DEFAULT_FONT_FAMILY_ID: &str = "__forktty_default_font__";
const SYSTEM_MONOSPACE_FONT_FAMILY_ID: &str = "__forktty_system_monospace__";
const INSTALLED_FONT_FAMILY_ID_PREFIX: &str = "font:";
const TERMINAL_THEME_ITEMS: &[(&str, &str)] = &[
    (config::TERMINAL_THEME_SYSTEM, "System"),
    (config::TERMINAL_THEME_CATPPUCCIN_MOCHA, "Catppuccin Mocha"),
    (config::TERMINAL_THEME_ROSE_PINE, "Rose Pine"),
    (config::TERMINAL_THEME_TOKYO_NIGHT, "Tokyo Night"),
    (config::TERMINAL_THEME_DRACULA, "Dracula"),
    (config::TERMINAL_THEME_GRUVBOX_DARK, "Gruvbox Dark"),
];
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
const GH_PR_VIEW_TIMEOUT: Duration = Duration::from_secs(8);
const GH_PR_VIEW_MAX_STDOUT_BYTES: u64 = 64 * 1024;

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
    ready_surfaces: Arc<Mutex<BTreeSet<String>>>,
}

impl GtkVteBackend {
    fn new(sender: mpsc::Sender<GtkTerminalCommand>) -> Self {
        Self {
            sender,
            surfaces: Arc::new(Mutex::new(BTreeMap::new())),
            ready_surfaces: Arc::new(Mutex::new(BTreeSet::new())),
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
        let surface_id = request.surface_id.clone();
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
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(&surface_id);
        if let Err(err) = self.send_command(GtkTerminalCommand::Spawn(request)) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                surfaces.remove(&surface_id);
            }
            if let Ok(mut ready_surfaces) = self.ready_surfaces.lock() {
                ready_surfaces.remove(&surface_id);
            }
            return Err(err);
        }
        Ok(())
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        {
            let surfaces = self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            if !surfaces.contains_key(surface_id) {
                return Err(TerminalError::NotFound(surface_id.to_string()));
            }
        }
        if !self
            .ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains(surface_id)
        {
            return Err(TerminalError::NotReady(surface_id.to_string()));
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
        let previous_size = (surface.cols, surface.rows);
        surface.cols = cols;
        surface.rows = rows;
        drop(surfaces);
        if let Err(err) = self.send_command(GtkTerminalCommand::Resize {
            surface_id: surface_id.to_string(),
            cols,
            rows,
        }) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                if let Some(surface) = surfaces.get_mut(surface_id) {
                    surface.cols = previous_size.0;
                    surface.rows = previous_size.1;
                }
            }
            return Err(err);
        }
        Ok(())
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let removed = surfaces
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        drop(surfaces);
        let was_ready = self
            .ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id);
        if let Err(err) = self.send_command(GtkTerminalCommand::Close {
            surface_id: surface_id.to_string(),
        }) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                surfaces.insert(surface_id.to_string(), removed);
            }
            if was_ready {
                if let Ok(mut ready_surfaces) = self.ready_surfaces.lock() {
                    ready_surfaces.insert(surface_id.to_string());
                }
            }
            return Err(err);
        }
        Ok(())
    }

    fn mark_surface_ready(&self, surface_id: &str) -> Result<(), TerminalError> {
        if !self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains_key(surface_id)
        {
            return Err(TerminalError::NotFound(surface_id.to_string()));
        }
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .insert(surface_id.to_string());
        Ok(())
    }

    fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id);
        Ok(())
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
    tooltip: &'static str,
    class_name: &'static str,
}

struct SidebarSnapshot {
    rows: Vec<SidebarWorkspaceRow>,
    active_workspace_id: Option<String>,
    active_workspace_name: Option<String>,
    active_status_label: Option<String>,
    active_full_path: Option<String>,
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

#[derive(Clone)]
struct TabBarUi {
    view: adw::TabView,
    tab_bar: adw::TabBar,
    /// Ordered vec of (workspace_id, TabPage) — mirrors the model order.
    pages: Rc<RefCell<Vec<(String, adw::TabPage)>>>,
    /// Guards reentrancy: true while a programmatic reconcile is running so
    /// `connect_selected_page_notify` does not recurse into select_sidebar_workspace.
    syncing: Rc<Cell<bool>>,
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
    /// Spawned child PID per surface, used to discover listening ports.
    surface_pids: Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    next_spawn_token: u64,
    #[cfg(feature = "browser")]
    browser_panes: Rc<RefCell<BTreeMap<String, Rc<crate::browser_pane::BrowserPaneWidget>>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SurfacePid {
    pid: i32,
    spawn_token: u64,
}

fn remove_surface_pid_for_spawn(
    pids: &mut BTreeMap<String, SurfacePid>,
    surface_id: &str,
    spawn_token: u64,
) -> bool {
    if !matches!(
        pids.get(surface_id),
        Some(entry) if entry.spawn_token == spawn_token
    ) {
        return false;
    }
    pids.remove(surface_id);
    true
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
            surface_pids: Rc::new(RefCell::new(BTreeMap::new())),
            next_spawn_token: 0,
            #[cfg(feature = "browser")]
            browser_panes: Rc::new(RefCell::new(BTreeMap::new())),
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
                } else {
                    eprintln!("Dropped send-text for unready terminal surface: {surface_id}");
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
                self.surface_pids.borrow_mut().remove(&surface_id);
                #[cfg(feature = "browser")]
                self.browser_panes.borrow_mut().remove(&surface_id);
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
        let spawn_state_for_error = self.state.clone();
        let spawn_state_for_ready = self.state.clone();
        let spawn_model_for_error = spawn_model.clone();
        let spawn_pids = self.surface_pids.clone();
        let spawn_pid_surface_id = request.surface_id.clone();
        self.next_spawn_token = self.next_spawn_token.checked_add(1).unwrap_or(1);
        let spawn_token = self.next_spawn_token;
        match spawn_vte_terminal_with_callback(&request, move |result| match result {
            Ok(pid) => {
                spawn_pids.borrow_mut().insert(
                    spawn_pid_surface_id.clone(),
                    SurfacePid {
                        pid: pid.0,
                        spawn_token,
                    },
                );
                if let Some(state) = &spawn_state_for_ready {
                    if let Err(err) = state.terminal.mark_surface_ready(&spawn_surface_id) {
                        eprintln!(
                            "Failed to mark terminal surface ready {}: {err}",
                            spawn_surface_id
                        );
                    }
                }
            }
            Err(err) => {
                record_terminal_spawn_failure(
                    &spawn_model,
                    &spawn_workspace_id,
                    &spawn_surface_id,
                    &err.to_string(),
                );
                if let Some(state) = &spawn_state_for_error {
                    let _ = state.terminal.close(&spawn_surface_id);
                }
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
                attach_vte_signal_handlers(
                    &widget,
                    &self.model,
                    &request,
                    &self.surface_pids,
                    spawn_token,
                );
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
        // Drop browser panes whose surfaces were removed from the model so the
        // webviews don't linger after a model-driven (e.g. socket) close.
        #[cfg(feature = "browser")]
        {
            let live_surface_ids = self
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
                .unwrap_or_default();
            self.browser_panes
                .borrow_mut()
                .retain(|surface_id, _| live_surface_ids.contains(surface_id));
        }
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
            queue_widget_focus(widget.clone().upcast());
        } else {
            // Browser panes are not in self.widgets; hand keyboard focus to the
            // pane's focus target so keyboard-only nav reaches the browser.
            #[cfg(feature = "browser")]
            if let Some(pane) = self.browser_panes.borrow().get(&focused_surface_id) {
                queue_widget_focus(pane.focus_target());
            }
        }
        self.last_layout_signature = Some(signature);
    }

    fn toggle_maximized_pane(&mut self) {
        self.maximized_pane = !self.maximized_pane;
        self.last_layout_signature = None;
        self.rebuild_layout();
    }

    fn model_focused_widget(&self) -> Option<VteTerminalWidget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.widgets.get(&surface_id).cloned()
    }

    fn gtk_focused_widget(&self) -> Option<VteTerminalWidget> {
        self.widgets
            .values()
            .find(|widget| widget.has_focus())
            .cloned()
    }

    // App-wide clipboard accelerators must only affect a terminal that currently
    // owns GTK focus; the model focus can legitimately be stale while dialogs or
    // search entries are active.
    fn copy_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.copy_clipboard_format(Format::Text);
        true
    }

    fn paste_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.paste_clipboard();
        true
    }

    fn select_all_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.select_all();
        true
    }

    fn reset_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        reset_and_redraw_terminal(&widget);
        true
    }

    // Explicit commands from the command palette intentionally target the active
    // terminal, because the palette itself owns GTK focus while the user chooses.
    fn copy_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.copy_clipboard_format(Format::Text);
        true
    }

    fn paste_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.paste_clipboard();
        true
    }

    fn select_all_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.select_all();
        true
    }

    fn reset_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        reset_and_redraw_terminal(&widget);
        true
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
            let Some(workspace) = model.active_workspace() else {
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
            // Browser surfaces are rendered by browser_panes and never get a
            // terminal backend; spawning a PTY for them would leak a hidden
            // shell process and emit bogus terminal status/port/close events.
            if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
                continue;
            }
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
            if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
                &surface,
                state.shell.clone(),
                state.socket_path.clone(),
            )) {
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
            .active_workspace()
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
        // Browser navigation only mutates a surface's url (same layout structure),
        // so the layout signature is unchanged and rebuild_layout does not fire.
        // Push the latest url into the live webview on each refresh tick instead,
        // and drop panes for surfaces that no longer exist to avoid leaking them.
        #[cfg(feature = "browser")]
        self.browser_panes.borrow_mut().retain(|surface_id, pane| {
            match model.surface(surface_id).map(|s| &s.kind) {
                Some(forktty_core::SurfaceKind::Browser { url, .. }) => {
                    // Safe to call every tick: BrowserPaneWidget edge-triggers on the
                    // last *requested* url, so an unchanged url is a no-op and user
                    // navigations (which only move the committed uri) are not reset.
                    pane.load_uri(url);
                    true
                }
                _ => false,
            }
        });
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
            PaneNode::Leaf { surface_id } => self.pane_widget_for(surface_id),
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

    fn pane_widget_for(&self, surface_id: &str) -> gtk::Widget {
        let kind = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.surface(surface_id).map(|s| s.kind.clone()));
        match kind {
            #[cfg(feature = "browser")]
            Some(forktty_core::SurfaceKind::Browser { url, profile }) => {
                self.browser_pane_widget(surface_id, &url, &profile.to_string())
            }
            #[cfg(not(feature = "browser"))]
            Some(forktty_core::SurfaceKind::Browser { .. }) => {
                browser_unavailable_placeholder(surface_id).upcast()
            }
            _ => self.terminal_pane_widget(surface_id),
        }
    }

    #[cfg(feature = "browser")]
    fn browser_pane_widget(&self, surface_id: &str, url: &str, profile_id: &str) -> gtk::Widget {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
            // Widget self-guards on the last requested url; calling unconditionally
            // is harmless and avoids the committed-uri divergence problem.
            pane.load_uri(url);
            return pane.widget();
        }
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(profile_id, url));
        // Address-bar Enter navigates via the model so socket + manual share one path.
        let model = self.model.clone();
        let id = surface_id.to_string();
        pane.connect_address_activate(move |text| {
            if let Ok(mut m) = model.lock() {
                let normalized = if forktty_core::has_uri_scheme(&text) {
                    text
                } else {
                    format!("https://{text}")
                };
                m.set_surface_url(&id, &normalized);
            }
        });
        // Keep model focus in sync when the browser pane gains focus, mirroring
        // the VTE has-focus handler, so focus-driven split/close target this pane.
        {
            let focus_model = self.model.clone();
            let focus_id = surface_id.to_string();
            pane.connect_focus_in(move || {
                if let Ok(mut m) = focus_model.lock() {
                    let _ = m.focus_surface(&focus_id);
                    let _ = m.mark_surface_unread(&focus_id, false);
                }
            });
        }
        // Wire the × button to the same confirmation flow terminal panes use.
        if let Some(state) = self.state.clone() {
            let parent = self.parent_window.clone();
            let sid_close = surface_id.to_string();
            pane.connect_close(move || {
                show_close_pane_confirmation(&parent, &state, &sid_close);
            });
        }
        let widget = pane.widget();
        self.browser_panes
            .borrow_mut()
            .insert(surface_id.to_string(), pane);
        widget
    }

    #[cfg(feature = "browser")]
    fn browser_pane(&self, surface_id: &str) -> Option<Rc<crate::browser_pane::BrowserPaneWidget>> {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id).cloned() {
            return Some(pane);
        }
        let (url, profile_id) = self.model.lock().ok().and_then(|model| {
            model
                .surface(surface_id)
                .and_then(|surface| match &surface.kind {
                    forktty_core::SurfaceKind::Browser { url, profile } => {
                        Some((url.clone(), profile.to_string()))
                    }
                    _ => None,
                })
        })?;
        let _ = self.browser_pane_widget(surface_id, &url, &profile_id);
        self.browser_panes.borrow().get(surface_id).cloned()
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
                        kind: forktty_core::SurfaceKind::Terminal,
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
    #[cfg(feature = "browser")]
    let open_browser = pane_action_button("web-browser-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    actions.append(&open_browser);
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
    #[cfg(feature = "browser")]
    let single_open_browser = pane_action_button("web-browser-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    single_pane_actions.append(&single_open_browser);
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
        #[cfg(feature = "browser")]
        {
            let state_for_browser = state.clone();
            let sid_browser = surface_id_owned.clone();
            open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_browser, &sid_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
            let state_for_single_browser = state.clone();
            let sid_single_browser = surface_id_owned.clone();
            single_open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_single_browser, &sid_single_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
        }
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
        #[cfg(feature = "browser")]
        {
            open_browser.set_sensitive(false);
            single_open_browser.set_sensitive(false);
        }
    }

    let motion = gtk::EventControllerMotion::new();
    {
        let actions_for_enter = actions.clone();
        motion.connect_enter(move |_, _, _| {
            actions_for_enter.add_css_class("revealed");
        });
    }
    {
        let actions_for_leave = actions.clone();
        motion.connect_leave(move |_| {
            actions_for_leave.remove_css_class("revealed");
        });
    }
    header.add_controller(motion);

    let focus = gtk::EventControllerFocus::new();
    {
        let actions_for_focus = actions.clone();
        focus.connect_enter(move |_| {
            actions_for_focus.add_css_class("focus-revealed");
        });
    }
    {
        let actions_for_focus = actions.clone();
        focus.connect_leave(move |_| {
            actions_for_focus.remove_css_class("focus-revealed");
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
        .active_workspace()
        .map(|workspace| workspace.focused_surface_id)
}

fn active_workspace_snapshot(state: &SocketAppState) -> Option<forktty_core::Workspace> {
    let model = state.model.lock().ok()?;
    model.active_workspace()
}

fn close_pane_confirmation_body(state: &SocketAppState, surface_id: &str) -> String {
    let target = state.model.lock().ok().and_then(|model| {
        let surface = model.surface(surface_id)?;
        let workspace_name = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == surface.workspace_id)
            .map(|workspace| workspace.name)
            .unwrap_or_else(|| surface.workspace_id.clone());
        Some(format!(
            "'{}' in workspace '{}' ({})",
            surface_title(surface),
            workspace_name,
            compact_path(&surface.cwd)
        ))
    });
    match target {
        Some(target) => {
            format!("Close pane {target}. Any process running inside it will be terminated.")
        }
        None => {
            format!("Close pane {surface_id}. Any process running inside it will be terminated.")
        }
    }
}

fn close_workspace_confirmation_body(name: &str, path: &Path) -> String {
    format!(
        "Close workspace '{name}' at {} and all panes inside it. Running terminal processes in this workspace will be closed.",
        compact_path(path)
    )
}

fn show_close_pane_confirmation(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    surface_id: &str,
) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    let body = close_pane_confirmation_body(&state, &surface_id);
    show_destructive_confirmation(parent, "Close Pane?", &body, "Close Pane", move || {
        focus_surface_and(&state, &surface_id, close_active_surface);
    });
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
    chrome.title.set_label(&title_text);
    chrome.title.set_tooltip_text(Some(&title_text));
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
    let workspace = model.active_workspace()?;
    let mut structure = String::new();
    layout_structure_signature(&workspace.pane_tree, &mut structure);
    let signature = format!(
        "{}:{}:focus({})",
        workspace.id, structure, workspace.focused_surface_id
    );
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
    let font = terminal_font_description(widget, &config);
    widget.add_css_class("vte-terminal");
    widget.set_font(Some(&font));
    widget.set_scrollback_lines(i64::from(config.appearance.scrollback_lines));
    widget.set_audible_bell(config.appearance.terminal_audible_bell);
    widget.set_mouse_autohide(true);
    widget.set_scroll_on_keystroke(true);
    widget.set_allow_hyperlink(true);
    widget.set_bold_is_bright(false);
    widget.set_cursor_blink_mode(CursorBlinkMode::System);
    widget.set_cursor_shape(CursorShape::Block);
    widget.set_word_char_exceptions("-#%&+,./:=?@_~");
    apply_terminal_colors(widget, terminal_colors_for_config(&config));
}

struct TerminalColors {
    background: &'static str,
    foreground: &'static str,
    bold: &'static str,
    cursor: &'static str,
    cursor_foreground: &'static str,
    highlight: &'static str,
    highlight_foreground: &'static str,
    ansi: [&'static str; 16],
}

fn terminal_colors_for_config(config: &config::AppConfig) -> &'static TerminalColors {
    let theme = config.appearance.terminal_theme.trim().to_ascii_lowercase();
    match theme.as_str() {
        config::TERMINAL_THEME_CATPPUCCIN_MOCHA => &CATPPUCCIN_MOCHA_TERMINAL_COLORS,
        config::TERMINAL_THEME_ROSE_PINE => &ROSE_PINE_TERMINAL_COLORS,
        config::TERMINAL_THEME_TOKYO_NIGHT => &TOKYO_NIGHT_TERMINAL_COLORS,
        config::TERMINAL_THEME_DRACULA => &DRACULA_TERMINAL_COLORS,
        config::TERMINAL_THEME_GRUVBOX_DARK => &GRUVBOX_DARK_TERMINAL_COLORS,
        _ if terminal_prefers_dark_palette(config) => &CATPPUCCIN_MOCHA_TERMINAL_COLORS,
        _ => &LIGHT_TERMINAL_COLORS,
    }
}

const CATPPUCCIN_MOCHA_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    bold: "#cdd6f4",
    cursor: "#f5e0dc",
    cursor_foreground: "#1e1e2e",
    highlight: "#f5e0dc",
    highlight_foreground: "#1e1e2e",
    ansi: [
        "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
        "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
    ],
};

const ROSE_PINE_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#191724",
    foreground: "#e0def4",
    bold: "#e0def4",
    cursor: "#524f67",
    cursor_foreground: "#e0def4",
    highlight: "#403d52",
    highlight_foreground: "#e0def4",
    ansi: [
        "#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
        "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
    ],
};

const TOKYO_NIGHT_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#1a1b26",
    foreground: "#c0caf5",
    bold: "#c0caf5",
    cursor: "#c0caf5",
    cursor_foreground: "#1a1b26",
    highlight: "#283457",
    highlight_foreground: "#c0caf5",
    ansi: [
        "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
        "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5",
    ],
};

const DRACULA_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#282a36",
    foreground: "#f8f8f2",
    bold: "#ffffff",
    cursor: "#f8f8f2",
    cursor_foreground: "#282a36",
    highlight: "#44475a",
    highlight_foreground: "#ffffff",
    ansi: [
        "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
        "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
    ],
};

const GRUVBOX_DARK_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#282828",
    foreground: "#ebdbb2",
    bold: "#fbf1c7",
    cursor: "#ebdbb2",
    cursor_foreground: "#282828",
    highlight: "#504945",
    highlight_foreground: "#fbf1c7",
    ansi: [
        "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
        "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
    ],
};

const LIGHT_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#fbfbfe",
    foreground: "#24292f",
    bold: "#0b0d12",
    cursor: "#0969da",
    cursor_foreground: "#ffffff",
    highlight: "#dbeafe",
    highlight_foreground: "#111827",
    ansi: [
        "#24292f", "#cf222e", "#1a7f37", "#9a6700", "#0969da", "#8250df", "#1b7c83", "#57606a",
        "#6e7781", "#a40e26", "#116329", "#7d4e00", "#0550ae", "#6639ba", "#0a6971", "#24292f",
    ],
};

fn apply_terminal_colors(widget: &VteTerminalWidget, colors: &TerminalColors) {
    let foreground = rgba(colors.foreground);
    let background = rgba(colors.background);
    let ansi = colors
        .ansi
        .iter()
        .map(|color| rgba(color))
        .collect::<Vec<_>>();
    let ansi_refs = ansi.iter().collect::<Vec<&gdk::RGBA>>();

    widget.set_colors(Some(&foreground), Some(&background), &ansi_refs);
    widget.set_color_bold(Some(&rgba(colors.bold)));
    widget.set_color_cursor(Some(&rgba(colors.cursor)));
    widget.set_color_cursor_foreground(Some(&rgba(colors.cursor_foreground)));
    widget.set_color_highlight(Some(&rgba(colors.highlight)));
    widget.set_color_highlight_foreground(Some(&rgba(colors.highlight_foreground)));
}

fn reset_and_redraw_terminal(widget: &VteTerminalWidget) {
    widget.reset(true, true);
    vte_send_text(widget, "\x0c");
}

fn terminal_prefers_dark_palette(config: &config::AppConfig) -> bool {
    let source = config.general.theme_source.trim().to_ascii_lowercase();
    match source.as_str() {
        "light" => false,
        "dark" => true,
        _ => adw::StyleManager::default().is_dark(),
    }
}

fn terminal_font_description(
    widget: &impl IsA<gtk::Widget>,
    config: &config::AppConfig,
) -> gtk::pango::FontDescription {
    let configured = config.appearance.font_family.trim();
    let family = if configured.is_empty() {
        default_terminal_font_family(&installed_font_families(widget))
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

fn installed_font_families(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    pango_font_families(widget, false)
}

fn installed_monospace_font_families(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    pango_font_families(widget, true)
}

fn pango_font_families(widget: &impl IsA<gtk::Widget>, monospace_only: bool) -> Vec<String> {
    let context = widget.as_ref().pango_context();
    let names = context
        .list_families()
        .into_iter()
        .filter(|family| !monospace_only || family.is_monospace())
        .map(|family| family.name().to_string());
    dedupe_font_family_names(names)
}

fn resolved_system_monospace_family(widget: &impl IsA<gtk::Widget>) -> Option<String> {
    let context = widget.as_ref().pango_context();
    let description = gtk::pango::FontDescription::from_string("monospace");
    context
        .load_font(&description)
        .and_then(|font| font.describe().family().map(|family| family.to_string()))
        .filter(|family| !family.trim().is_empty())
        .or_else(|| installed_monospace_font_families(widget).into_iter().next())
}

fn dedupe_font_family_names(raw_names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = BTreeSet::new();
    for name in raw_names {
        let name = name.trim();
        if !name.is_empty() {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}

fn attach_vte_signal_handlers(
    widget: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
    surface_pids: &Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    spawn_token: u64,
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
            // The user may have closed this pane before the bell signal
            // drained from VTE. Don't materialize a notification that points
            // at a surface the model no longer knows about — the row would
            // render as a dead-end click target.
            if model.surface(&surface_id).is_none() {
                return;
            }
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
    let exit_surface_pids = surface_pids.clone();
    // VTE emits child-exited exactly once in normal teardown but can in rare
    // cases (force-kill, fast respawn) fire twice. A single-shot latch keeps
    // the status + notification idempotent per surface.
    let exit_fired = Rc::new(Cell::new(false));
    widget.connect_child_exited(move |_, status| {
        if exit_fired.replace(true) {
            return;
        }
        let mut pids = exit_surface_pids.borrow_mut();
        if !remove_surface_pid_for_spawn(&mut pids, &surface_id, spawn_token) {
            return;
        }
        drop(pids);
        if let Ok(mut model) = exit_model.lock() {
            if model.surface(&surface_id).is_none() {
                return;
            }
            if status == 0 {
                let _ = model.set_status(
                    &workspace_id,
                    surface_status_key(&surface_id),
                    "Terminal",
                    "Closed",
                    None,
                );
                return;
            }
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

    if let Some(notification) =
        create_prompt_notification_if_surface_exists(model, workspace_id, surface_id, body)
    {
        dispatch_notification_with_loaded_config(&notification);
    }
}

fn create_prompt_notification_if_surface_exists(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    body: &str,
) -> Option<NotificationItem> {
    let mut model = model.lock().ok()?;
    let surface = model.surface(surface_id)?;
    if surface.workspace_id != workspace_id {
        return None;
    }
    Some(model.create_notification(
        "Terminal prompt",
        body,
        NotificationKind::Prompt,
        Some(workspace_id.to_string()),
        Some(surface_id.to_string()),
    ))
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
        .default_width(400)
        .default_height(200)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title_label = gtk::Label::builder().label(title).xalign(0.0).build();
    title_label.add_css_class("ft-dialog-title");
    header.append(&title_label);

    let body_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body_container.add_css_class("ft-dialog-body");
    let body_label = gtk::Label::builder()
        .label(body)
        .xalign(0.0)
        .wrap(true)
        .build();
    body_label.add_css_class("ft-dialog-confirm-body");
    body_container.append(&body_label);

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
    content.append(&body_container);
    content.append(&footer);
    dialog.set_default_widget(Some(&cancel));
    dialog.set_child(Some(&content));
    dialog.present();
    cancel.grab_focus();
}

fn show_rename_workspace_dialog<W>(
    parent: &W,
    state: &SocketAppState,
    workspace_id: &str,
    current_name: &str,
) where
    W: IsA<gtk::Window>,
{
    let dialog = gtk::Window::builder()
        .title("Rename Workspace")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(190)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Rename Workspace")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Choose a short name that is easy to recognize in the sidebar and status bar.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("ft-dialog-body");
    let entry = gtk::Entry::builder()
        .text(current_name)
        .placeholder_text("Workspace name")
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Workspace name")]);
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");
    body.append(&entry);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let rename = gtk::Button::with_label("Rename");
    rename.add_css_class("suggested-action");
    rename.set_sensitive(false);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&rename);

    let current_name_owned = current_name.to_string();
    let status_for_change = status.clone();
    let rename_for_change = rename.clone();
    entry.connect_changed(move |entry| {
        let candidate = entry.text();
        let trimmed = candidate.trim();
        let valid = !trimmed.is_empty() && trimmed != current_name_owned;
        rename_for_change.set_sensitive(valid);
        if trimmed.is_empty() {
            set_status_message(
                &status_for_change,
                "Workspace name cannot be empty.",
                StatusKind::Error,
            );
        } else {
            clear_status_message(&status_for_change);
        }
    });

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let state_for_rename = state.clone();
    let workspace_id_for_rename = workspace_id.to_string();
    let dialog_for_rename = dialog.clone();
    let status_for_rename = status.clone();
    let entry_for_rename = entry.clone();
    rename.connect_clicked(move |_| {
        match rename_workspace_gtk(
            &state_for_rename,
            &workspace_id_for_rename,
            entry_for_rename.text().as_str(),
        ) {
            Ok(()) => dialog_for_rename.close(),
            Err(err) => set_status_message(&status_for_rename, &err, StatusKind::Error),
        }
    });

    let rename_for_activate = rename.clone();
    entry.connect_activate(move |_| {
        if rename_for_activate.is_sensitive() {
            rename_for_activate.emit_clicked();
        }
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);
    content.append(&footer);
    dialog.set_default_widget(Some(&rename));
    dialog.set_child(Some(&content));
    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
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
    let badge = button
        .child()
        .and_then(|child| child.downcast::<gtk::Overlay>().ok())
        .and_then(|overlay| overlay.first_child())
        .and_then(|first| first.next_sibling())
        .and_then(|child| child.downcast::<gtk::Label>().ok());

    let (total, unread) = state
        .model
        .lock()
        .ok()
        .map(|model| {
            (
                model.list_notifications().len(),
                model.unread_notification_count(),
            )
        })
        .unwrap_or((0, 0));
    if unread == 0 {
        button.remove_css_class("needs-attention");
        if let Some(badge) = &badge {
            badge.set_visible(false);
        }
        let label = if total == 0 {
            "Notifications".to_string()
        } else if total == 1 {
            "Notifications: 1 read".to_string()
        } else {
            format!("Notifications: {total} read")
        };
        button.set_tooltip_text(Some(&format!("{label} (Ctrl+Shift+M)")));
        set_accessible_button_text(button, &label, Some("Ctrl+Shift+M"));
    } else {
        button.add_css_class("needs-attention");
        if let Some(badge) = &badge {
            let display = if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            badge.set_text(&display);
            badge.set_visible(true);
        }
        let label = if unread == 1 {
            "Notifications: 1 unread".to_string()
        } else {
            format!("Notifications: {unread} unread")
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

fn focus_workspace(state: &SocketAppState, workspace_id: &str) -> bool {
    match select_workspace_with_terminal(state, workspace_id) {
        Ok(selected) => selected,
        Err(err) => {
            eprintln!("Failed to spawn workspace terminal: {err}");
            create_global_notification(
                state,
                "Workspace Switch Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            false
        }
    }
}

fn select_workspace_with_terminal(
    state: &SocketAppState,
    workspace_id: &str,
) -> Result<bool, TerminalError> {
    let (selected_id, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let previous_active_id = model.active_workspace_id();
        let Some(selected) = model.select_workspace(WorkspaceSelector::Id(workspace_id)) else {
            return Ok(false);
        };
        (selected.id, previous_active_id)
    };

    if let Err(err) = spawn_focused_surface_if_needed(state) {
        if previous_active_id.as_deref() != Some(selected_id.as_str()) {
            if let Some(previous_active_id) = previous_active_id.as_deref() {
                if let Ok(mut model) = state.model.lock() {
                    let _ = model.select_workspace(WorkspaceSelector::Id(previous_active_id));
                }
            }
        }
        return Err(err);
    }

    Ok(true)
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
    let workspace_name = workspace.name.clone();
    let is_active = workspace.active;
    let worktree_name = workspace.worktree_name.clone();
    let working_dir = workspace.working_dir.clone();

    if !is_active {
        let state_ = state.clone();
        let controller_ = controller.clone();
        let ws_id = workspace_id.clone();
        // `go-jump-symbolic` reads as "navigate to" rather than the
        // submenu-style chevron that `go-next-symbolic` implies.
        add_context_menu_item(
            &menu,
            &popover,
            "go-jump-symbolic",
            "Focus Workspace",
            false,
            move || {
                if focus_workspace(&state_, &ws_id) {
                    controller_.borrow_mut().rebuild_layout();
                }
            },
        );
        add_context_menu_separator(&menu);
    }

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    let ws_name = workspace_name.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "document-edit-symbolic",
        "Rename Workspace...",
        false,
        move || show_rename_workspace_dialog(&parent_, &state_, &ws_id, &ws_name),
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "view-dual-symbolic",
        "Split Right",
        false,
        move || {
            if focus_workspace(&state_, &ws_id) {
                split_active_surface(&state_, SplitAxis::Horizontal);
            }
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
            if focus_workspace(&state_, &ws_id) {
                split_active_surface(&state_, SplitAxis::Vertical);
            }
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
            if focus_workspace(&state_, &ws_id) {
                show_worktree_dialog(&parent_, &state_);
            }
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
        let ws_id = workspace_id.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "emblem-ok-symbolic",
            "Merge Worktree",
            false,
            move || {
                if !focus_workspace(&state_, &ws_id) {
                    return;
                }
                match merge_worktree_from_gtk(&state_, &name_) {
                    Ok(msg) => create_local_notification(&state_, "Worktree Merged", &msg),
                    Err(err) => create_local_notification(&state_, "Merge Failed", &err),
                }
            },
        );

        let state_ = state.clone();
        let parent_ = parent.clone();
        let ws_id = workspace_id.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "user-trash-symbolic",
            "Remove Worktree",
            true,
            move || {
                let state_confirm = state_.clone();
                let ws_id_confirm = ws_id.clone();
                let name_confirm = name_.clone();
                show_destructive_confirmation(
                    &parent_,
                    "Remove Worktree?",
                    &format!(
                        "Remove worktree '{name_confirm}' and close its ForkTTY workspace. The git branch is left intact."
                    ),
                    "Remove Worktree",
                    move || {
                        if !focus_workspace(&state_confirm, &ws_id_confirm) {
                            return;
                        }
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
    let ws_name = workspace_name.clone();
    let ws_path = working_dir.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Workspace",
        true,
        move || {
            let state_confirm = state_.clone();
            let ws_id_confirm = ws_id.clone();
            let body = close_workspace_confirmation_body(&ws_name, &ws_path);
            show_destructive_confirmation(
                &parent_,
                "Close Workspace?",
                &body,
                "Close Workspace",
                move || {
                    if focus_workspace(&state_confirm, &ws_id_confirm) {
                        close_active_workspace(&state_confirm);
                    }
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

    let terminal_for_select = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-select-all-symbolic",
        "Select All",
        false,
        move || terminal_for_select.select_all(),
    );

    let terminal_for_reset = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-clear-all-symbolic",
        "Reset and Clear",
        false,
        move || reset_and_redraw_terminal(&terminal_for_reset),
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
        let (popover_x, popover_y) = widget_for_menu
            .translate_coordinates(&parent_for_menu, x, y)
            .unwrap_or((x, y));
        popover.set_parent(&parent_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            popover_x.round() as i32,
            popover_y.round() as i32,
            1,
            1,
        )));
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
        if paned.root().is_none() {
            ready.set(true);
            return glib::ControlFlow::Break;
        }
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

fn queue_widget_focus(widget: gtk::Widget) {
    glib::idle_add_local_once(move || {
        if widget.root().is_some() {
            widget.grab_focus();
        }
    });
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

#[cfg(not(feature = "browser"))]
fn browser_unavailable_placeholder(surface_id: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.set_hexpand(true);
    b.set_vexpand(true);
    b.append(&gtk::Label::new(Some(&format!(
        "Browser pane ({surface_id}) — built without the `browser` feature"
    ))));
    b
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
                description: terminal_failure_guidance(&status.value),
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
        title: "Terminal Waiting to Start",
        description: format!(
            "This pane is waiting for its terminal process. Restart the pane if it stays here. Diagnostic ID: {surface_id}."
        ),
        can_restart: true,
    }
}

fn terminal_failure_guidance(message: &str) -> String {
    let summary = truncate_single_line(message, 180);
    let lower = message.to_ascii_lowercase();
    let hint = if lower.contains("no such file or directory")
        || lower.contains("not found")
        || lower.contains("permission denied")
    {
        "Check the shell path and permissions in Settings, then restart this pane."
    } else if lower.contains("vte") || lower.contains("pty") {
        "The VTE/PTY backend failed. Restart this pane after checking the terminal backend."
    } else {
        "Check Settings, then restart this pane."
    };
    format!("{summary}. {hint}")
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

fn surface_title(surface: &Surface) -> String {
    let title = surface.title.trim();
    if !title.is_empty() && title != "shell" {
        return title.to_string();
    }
    if let Some(name) = surface
        .cwd
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
    {
        return name.to_string();
    }
    "Terminal".to_string()
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
    register_app_icon();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
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
    let backend = Arc::new(GtkVteBackend::new(terminal_tx));
    #[cfg(feature = "browser")]
    let (browser_cmd_tx, browser_cmd_rx) =
        async_channel::unbounded::<forktty_core::BrowserCommand>();
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path);
    #[cfg(feature = "browser")]
    let state = state.with_browser_cmd(browser_cmd_tx);
    if let Some(message) = config_load_warning.as_deref() {
        create_global_notification(&state, "Config Issue", message, NotificationKind::Error);
    }
    let ui_alive = Rc::new(Cell::new(true));

    let header = adw::HeaderBar::new();
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
    let brand_cursor = gtk::Label::builder().label("_").xalign(0.0).build();
    brand_cursor.add_css_class("app-brand-name");
    brand_cursor.add_css_class("app-brand-cursor");
    let brand_alpha_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    brand_alpha_dot.add_css_class("app-brand-tag-dot");
    brand_alpha_dot.set_valign(gtk::Align::Center);
    brand_alpha_dot.set_size_request(5, 5);
    let brand_alpha_label = gtk::Label::builder().label("ALPHA").build();
    brand_alpha_label.add_css_class("app-brand-tag-label");
    let brand_alpha = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    brand_alpha.add_css_class("app-brand-tag");
    brand_alpha.set_valign(gtk::Align::Center);
    brand_alpha.set_tooltip_text(Some("Pre-release build"));
    brand_alpha.append(&brand_alpha_dot);
    brand_alpha.append(&brand_alpha_label);
    brand_wordmark.append(&brand_name);
    brand_wordmark.append(&brand_cursor);
    brand.append(&brand_logo);
    brand.append(&brand_wordmark);
    brand.append(&brand_alpha);
    let brand_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    brand_separator.add_css_class("header-action-separator");
    header.pack_start(&brand);
    header.pack_start(&brand_separator);

    let workspace_title = gtk::Button::builder().label("").has_frame(false).build();
    workspace_title.add_css_class("flat");
    workspace_title.add_css_class("app-header-title");
    workspace_title.set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
    workspace_title.set_sensitive(false);
    set_accessible_button_text(&workspace_title, "No active workspace", None);
    header.set_title_widget(Some(&workspace_title));

    let command_palette = gtk::Button::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Command Palette (Ctrl+Shift+P)")
        .build();
    let notif_overlay = gtk::Overlay::new();
    notif_overlay.set_halign(gtk::Align::Center);
    notif_overlay.set_valign(gtk::Align::Center);
    let notif_icon = gtk::Image::from_icon_name("preferences-system-notifications-symbolic");
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
    let status_location = gtk::Button::builder().label("").has_frame(false).build();
    status_location.add_css_class("flat");
    status_location.add_css_class("status-location");
    status_location.set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
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

    // ── Workspace tab bar ──────────────────────────────────────────────────
    // AdwTabView is used selector-only: each TabPage wraps a tiny empty Box
    // child. The real terminal content stays in terminal_stack.  Only the
    // AdwTabBar widget is inserted into the layout; the TabView itself stays
    // off-tree (just `tab_bar.set_view(Some(&view))`).
    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(&tab_view));
    tab_bar.set_autohide(false);
    tab_bar.set_expand_tabs(false);
    tab_bar.add_css_class("workspace-tabbar");
    let tab_new_btn = gtk::Button::builder()
        .icon_name("tab-new-symbolic")
        .tooltip_text("New Workspace")
        .build();
    tab_new_btn.add_css_class("flat");
    tab_new_btn.set_action_name(Some("app.new-workspace"));
    set_accessible_button_text(&tab_new_btn, "New Workspace", None);
    tab_bar.set_end_action_widget(Some(&tab_new_btn));
    tab_bar.set_visible(app_config.appearance.show_workspace_tabs);

    let tab_bar_ui = TabBarUi {
        view: tab_view,
        tab_bar: tab_bar.clone(),
        pages: Rc::new(RefCell::new(Vec::new())),
        syncing: Rc::new(Cell::new(false)),
    };

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("app-root");
    content.append(&header);
    content.append(&tab_bar);
    content.append(&paned);
    content.append(&status_bar);

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
        .content(&content)
        .build();
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
        status_location: status_location.clone(),
        pane_status: pane_status.clone(),
        last_signature: Rc::new(RefCell::new(None::<String>)),
        context_menu_open: Rc::new(Cell::new(false)),
        context_popover: Rc::new(RefCell::new(None)),
    };

    // ── Wire tab bar signals ───────────────────────────────────────────────
    {
        let state_for_tab = state.clone();
        let controller_for_tab = controller.clone();
        let sidebar_ui_for_tab = sidebar_ui.clone();
        let tab_bar_ui_for_sel = tab_bar_ui.clone();
        tab_bar_ui.view.connect_selected_page_notify(move |view| {
            if tab_bar_ui_for_sel.syncing.get() {
                return;
            }
            let Some(page) = view.selected_page() else {
                return;
            };
            // Find the workspace id that corresponds to this page.
            let workspace_id = tab_bar_ui_for_sel
                .pages
                .borrow()
                .iter()
                .find(|(_, p)| p == &page)
                .map(|(id, _)| id.clone());
            let Some(workspace_id) = workspace_id else {
                return;
            };
            select_sidebar_workspace(&state_for_tab, &workspace_id, &controller_for_tab);
            schedule_sidebar_refresh(
                sidebar_ui_for_tab.clone(),
                tab_bar_ui_for_sel.clone(),
                state_for_tab.clone(),
                controller_for_tab.clone(),
            );
        });
    }
    {
        let state_for_close = state.clone();
        let window_for_close = window.clone();
        let tab_bar_ui_for_close = tab_bar_ui.clone();
        tab_bar_ui.view.connect_close_page(move |_view, page| {
            // Look up workspace info for the confirmation dialog.
            let workspace_id = tab_bar_ui_for_close
                .pages
                .borrow()
                .iter()
                .find(|(_, p)| p == page)
                .map(|(id, _)| id.clone());
            let Some(workspace_id) = workspace_id else {
                return glib::Propagation::Stop;
            };
            let (ws_name, ws_path) = {
                let Ok(model) = state_for_close.model.lock() else {
                    return glib::Propagation::Stop;
                };
                let ws = model
                    .list_workspaces()
                    .into_iter()
                    .find(|w| w.id == workspace_id);
                match ws {
                    Some(w) => (w.name.clone(), w.working_dir.clone()),
                    None => return glib::Propagation::Stop,
                }
            };
            let body = close_workspace_confirmation_body(&ws_name, &ws_path);
            let state_confirm = state_for_close.clone();
            show_destructive_confirmation(
                &window_for_close,
                "Close Workspace?",
                &body,
                "Close Workspace",
                move || {
                    if focus_workspace(&state_confirm, &workspace_id) {
                        close_active_workspace(&state_confirm);
                    }
                },
            );
            // Return Stop so AdwTabView does not remove the page itself.
            // The page will disappear on the next model→tabbar reconcile.
            glib::Propagation::Stop
        });
    }

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
    refresh_sidebar(&sidebar_ui, &tab_bar_ui, &state, &controller, true);
    refresh_tabbar(&tab_bar_ui, &state);
    let state_for_sidebar = state.clone();
    let controller_for_sidebar = controller.clone();
    let sidebar_ui_for_timer = sidebar_ui.clone();
    let tab_bar_ui_for_timer = tab_bar_ui.clone();
    let state_for_tabbar_timer = state.clone();
    let alive_for_sidebar_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if !alive_for_sidebar_timer.get() {
            return glib::ControlFlow::Break;
        }
        refresh_sidebar(
            &sidebar_ui_for_timer,
            &tab_bar_ui_for_timer,
            &state_for_sidebar,
            &controller_for_sidebar,
            false,
        );
        refresh_tabbar(&tab_bar_ui_for_timer, &state_for_tabbar_timer);
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
        if pr_lookup_enabled() {
            spawn_pr_refresh(pr_model_for_timer.clone(), pr_in_flight_for_timer.clone());
        } else {
            clear_pr_hints(&pr_model_for_timer);
        }
        glib::ControlFlow::Continue
    });
    install_session_autosave(&state);

    let terminal_stack_for_settings = terminal_stack.borrow().clone();
    let settings_apply = settings_apply_callback(
        &paned,
        &sidebar_shell,
        &tab_bar_ui.tab_bar,
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

    let state_for_bootstrap = state.clone();
    let controller_for_bootstrap = controller.clone();
    let sidebar_ui_for_bootstrap = sidebar_ui.clone();
    let tab_bar_ui_for_bootstrap = tab_bar_ui.clone();
    let pr_model_for_bootstrap = state.model.clone();
    let pr_in_flight_for_bootstrap = pr_in_flight.clone();
    let enable_pr_lookup_on_startup = app_config.general.enable_pr_lookup;
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
            &tab_bar_ui_for_bootstrap,
            &state_for_bootstrap,
            &controller_for_bootstrap,
            true,
        );
        refresh_tabbar(&tab_bar_ui_for_bootstrap, &state_for_bootstrap);
        if enable_pr_lookup_on_startup {
            spawn_pr_refresh(pr_model_for_bootstrap, pr_in_flight_for_bootstrap);
        }
        start_socket_server(state_for_bootstrap.clone());
    });
}

fn settings_apply_callback(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    tab_bar: &adw::TabBar,
    terminal_stack: &gtk::Box,
    controller: &Rc<RefCell<VteController>>,
) -> SettingsApplyCallback {
    let paned = paned.clone();
    let sidebar_shell = sidebar_shell.clone();
    let tab_bar = tab_bar.clone();
    let terminal_stack = terminal_stack.clone();
    let controller = controller.clone();
    Rc::new(move |config: &config::AppConfig| {
        apply_color_scheme(config);
        apply_sidebar_position(
            &paned,
            &sidebar_shell,
            &terminal_stack,
            &config.appearance.sidebar_position,
        );
        sidebar_shell.set_visible(config.appearance.sidebar_visible);
        tab_bar.set_visible(config.appearance.show_workspace_tabs);
        let model = {
            let controller = controller.borrow();
            for widget in controller.widgets.values() {
                apply_vte_appearance(widget);
            }
            controller.model.clone()
        };
        if !config.general.enable_pr_lookup {
            clear_pr_hints(&model);
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

fn socket_path_from_env(socket_env: Option<String>) -> PathBuf {
    socket_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path)
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
    !shell.is_empty() && is_executable_file(Path::new(shell))
}

fn restore_or_bootstrap_workspaces(state: &SocketAppState, cwd: PathBuf) -> Result<(), String> {
    match session::load_session() {
        Ok(Some(data)) if !data.workspaces.is_empty() => {
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

fn register_app_icon() {
    gtk::Window::set_default_icon_name("forktty");
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let icon_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packaging")
        .join("linux")
        .join("icons");
    if icon_dir.is_dir() {
        gtk::IconTheme::for_display(&display).add_search_path(icon_dir);
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

/// Recompute each workspace's listening-port hint from its surfaces' child PIDs.
/// Runs on a slow cadence; the sidebar refresh timer renders the updated model.
fn refresh_listening_ports(
    controller: &Rc<RefCell<VteController>>,
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

struct AtomicBoolReset {
    flag: Arc<AtomicBool>,
}

impl Drop for AtomicBoolReset {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

fn pr_lookup_enabled() -> bool {
    config::load_config()
        .map(|config| config.general.enable_pr_lookup)
        .unwrap_or(false)
}

fn clear_pr_hints(model: &Arc<Mutex<WorkspaceModel>>) {
    let Ok(mut model) = model.lock() else {
        return;
    };
    let workspace_ids = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| workspace.id)
        .collect::<Vec<_>>();
    for workspace_id in workspace_ids {
        model.set_pr(&workspace_id, None);
    }
}

fn trusted_command_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

fn child_stdout(mut child: std::process::Child) -> Option<String> {
    let stdout = child.stdout.take()?;
    let mut buffer = String::new();
    stdout
        .take(GH_PR_VIEW_MAX_STDOUT_BYTES)
        .read_to_string(&mut buffer)
        .ok()?;
    Some(buffer)
}

fn run_gh_pr_view(dir: &Path) -> Option<String> {
    let gh = trusted_command_path("gh")?;
    let mut child = Command::new(gh)
        .args(["pr", "view", "--json", "number,state,isDraft,url"])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = wait_with_timeout(&mut child, GH_PR_VIEW_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    child_stdout(child)
}

/// Kick off a background PR refresh unless one is already running.
fn spawn_pr_refresh(model: Arc<Mutex<WorkspaceModel>>, in_flight: Arc<AtomicBool>) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let _reset = AtomicBoolReset { flag: in_flight };
        refresh_pull_requests(model);
    });
}

/// Resolve each workspace's linked PR via `gh`, writing results into the shared
/// model. `gh` makes a network call, so this runs on a worker thread, never the
/// GTK main loop.
fn refresh_pull_requests(model: Arc<Mutex<WorkspaceModel>>) {
    if !pr_lookup_enabled() {
        clear_pr_hints(&model);
        return;
    }
    let targets: Vec<(String, PathBuf)> = match model.lock() {
        Ok(model) => model
            .list_workspaces()
            .into_iter()
            .map(|workspace| (workspace.id, workspace.working_dir))
            .collect(),
        Err(_) => return,
    };
    for (workspace_id, working_dir) in targets {
        let pr = resolve_pr(&working_dir);
        if !pr_lookup_enabled() {
            clear_pr_hints(&model);
            return;
        }
        if let Ok(mut model) = model.lock() {
            model.set_pr(&workspace_id, pr);
        }
    }
}

/// Run `gh pr view` in `dir` and parse the result. Returns `None` when there is
/// no PR for the checked-out branch, `gh` is absent, or `dir` is not a GitHub
/// checkout.
fn resolve_pr(dir: &Path) -> Option<forktty_core::pr::PrInfo> {
    let stdout = run_gh_pr_view(dir)?;
    forktty_core::pr::parse_pr_view(&stdout)
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
    add_action(app, "copy", {
        let controller = controller.clone();
        move || {
            controller.borrow().copy_focused_terminal();
        }
    });
    add_action(app, "paste", {
        let controller = controller.clone();
        move || {
            controller.borrow().paste_focused_terminal();
        }
    });
    add_action(app, "select-all", {
        let controller = controller.clone();
        move || {
            controller.borrow().select_all_focused_terminal();
        }
    });
    add_action(app, "reset-terminal", {
        let controller = controller.clone();
        move || {
            controller.borrow().reset_focused_terminal();
        }
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
    app.set_accels_for_action("app.copy", &["<Control><Shift>C"]);
    app.set_accels_for_action("app.paste", &["<Control><Shift>V"]);
    app.set_accels_for_action("app.select-all", &["<Control><Shift>A"]);
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
    tab_bar_ui: TabBarUi,
    state: SocketAppState,
    controller: Rc<RefCell<VteController>>,
) {
    glib::idle_add_local_once(move || {
        refresh_sidebar(&ui, &tab_bar_ui, &state, &controller, true);
        refresh_tabbar(&tab_bar_ui, &state);
    });
}

/// Reconcile the AdwTabView's pages to match the current model order.
/// Source of truth is `sidebar_snapshot`; this function is the tab-bar
/// equivalent of the sidebar portion of `refresh_sidebar`.
fn refresh_tabbar(tab_bar_ui: &TabBarUi, state: &SocketAppState) {
    let snapshot = sidebar_snapshot(state);
    let view = &tab_bar_ui.view;

    // Set syncing=true for the entire reconcile so the selected-page signal
    // handler does not re-enter select_sidebar_workspace.
    tab_bar_ui.syncing.set(true);

    // ── Remove pages whose workspace was deleted ──────────────────────────
    let model_ids: Vec<&str> = snapshot
        .rows
        .iter()
        .map(|r| r.workspace.id.as_str())
        .collect();
    let mut pages = tab_bar_ui.pages.borrow_mut();
    pages.retain(|(id, page)| {
        if model_ids.contains(&id.as_str()) {
            true
        } else {
            // Close and immediately finish so the page is removed from the view.
            view.close_page(page);
            view.close_page_finish(page, true);
            false
        }
    });

    // ── Add pages for new workspaces ──────────────────────────────────────
    for row in &snapshot.rows {
        let id = &row.workspace.id;
        if !pages.iter().any(|(pid, _)| pid == id) {
            let placeholder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let page = view.append(&placeholder);
            page.set_title(&row.workspace.name);
            pages.push((id.clone(), page));
        }
    }

    // ── Reorder pages to match model order ───────────────────────────────
    for (target_pos, row) in snapshot.rows.iter().enumerate() {
        if let Some(page) = pages
            .iter()
            .find(|(id, _)| id == &row.workspace.id)
            .map(|(_, p)| p.clone())
        {
            view.reorder_page(&page, target_pos as i32);
        }
    }

    // ── Update titles ─────────────────────────────────────────────────────
    for row in &snapshot.rows {
        if let Some((_, page)) = pages.iter().find(|(id, _)| id == &row.workspace.id) {
            if page.title().as_str() != row.workspace.name.as_str() {
                page.set_title(&row.workspace.name);
            }
        }
    }

    // ── Select the active workspace ───────────────────────────────────────
    if let Some(active_id) = snapshot.active_workspace_id.as_deref() {
        if let Some((_, page)) = pages.iter().find(|(id, _)| id == active_id) {
            let page = page.clone();
            view.set_selected_page(&page);
        }
    }

    drop(pages);
    tab_bar_ui.syncing.set(false);
}

fn refresh_sidebar(
    ui: &SidebarUi,
    tab_bar_ui: &TabBarUi,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    force: bool,
) {
    let snapshot = sidebar_snapshot(state);
    if let Some(name) = snapshot.active_workspace_name.as_deref() {
        ui.workspace_title.set_label(name);
        ui.workspace_title
            .set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
        set_accessible_button_text(
            &ui.workspace_title,
            &format!("Active workspace: {name}"),
            None,
        );
        ui.workspace_title.set_sensitive(true);
    } else {
        ui.workspace_title.set_label("No workspace");
        ui.workspace_title
            .set_tooltip_text(Some("No active workspace"));
        set_accessible_button_text(&ui.workspace_title, "No active workspace", None);
        ui.workspace_title.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_status_label.as_deref() {
        ui.status_location.set_label(label);
        if let Some(path) = snapshot.active_full_path.as_deref() {
            ui.status_location.set_tooltip_text(Some(&format!(
                "Switch workspace (Ctrl+Shift+P)\nFull path: {path}"
            )));
        } else {
            ui.status_location
                .set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
        }
        ui.status_location
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Workspace location: {label}"
            ))]);
        ui.status_location.set_sensitive(true);
    } else {
        ui.status_location.set_label("");
        ui.status_location
            .set_tooltip_text(Some("No active workspace location"));
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
            accessible_label.push_str(&format!(". {}", status.tooltip));
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
            .spacing(2)
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
            status_badge.set_tooltip_text(Some(status.tooltip));
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
            count_badge.set_valign(gtk::Align::Center);
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
            tooltip.push_str(status.tooltip);
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
        let tab_bar_ui_for_click = tab_bar_ui.clone();
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
                tab_bar_ui_for_click.clone(),
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
        let tab_bar_ui_for_key = tab_bar_ui.clone();
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
                tab_bar_ui_for_key.clone(),
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
        let tab_bar_ui_for_menu = tab_bar_ui.clone();
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
            let tab_bar_ui_for_closed = tab_bar_ui_for_menu.clone();
            popover.connect_closed(move |popover| {
                let is_current = ui_for_closed
                    .context_popover
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| current == popover);
                if is_current {
                    ui_for_closed.context_menu_open.set(false);
                    ui_for_closed.context_popover.borrow_mut().take();
                    schedule_sidebar_refresh(
                        ui_for_closed.clone(),
                        tab_bar_ui_for_closed.clone(),
                        state_for_closed.clone(),
                        controller_for_closed.clone(),
                    );
                }
                if popover.parent().is_some() {
                    popover.unparent();
                }
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
            active_workspace_id: None,
            active_workspace_name: None,
            active_status_label: None,
            active_full_path: None,
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
        let surfaces = model.list_surfaces(Some(&workspace.id));
        let surface_count = surfaces.len();
        let ssh_host = surfaces.iter().find_map(|s| {
            if let forktty_core::SurfaceKind::Ssh { host } = &s.kind {
                Some(host.clone())
            } else {
                None
            }
        });
        let meta = workspace_meta_line(&workspace, ssh_host.as_deref());
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
    let active_full_path = active_workspace.map(|workspace| {
        model
            .surface(&workspace.focused_surface_id)
            .map(|surface| surface.cwd.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.working_dir.to_string_lossy().to_string())
    });
    let active_pane_label = active_workspace.and_then(|workspace| {
        let leaves = collect_leaves(&workspace.pane_tree);
        let pane_count = leaves.len();
        let index = leaves
            .iter()
            .position(|surface_id| surface_id == &workspace.focused_surface_id)?;
        let surface = model.surface(&workspace.focused_surface_id);
        let title = surface
            .map(surface_title)
            .unwrap_or_else(|| "Terminal".to_string());
        let compact_cwd = surface
            .map(|surface| compact_path(&surface.cwd))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        let full_cwd = surface
            .map(|surface| surface.cwd.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.working_dir.to_string_lossy().to_string());
        let title_is_cwd_echo = title == compact_cwd.as_str()
            || title == full_cwd.as_str()
            || full_cwd.ends_with(&format!("/{title}"));
        if pane_count <= 1 {
            // With a single pane the "Pane 1/1" prefix is noise; the cwd is
            // already shown in status_location. Only surface a distinct title.
            if title == "Terminal" || title_is_cwd_echo {
                return None;
            }
            return Some(title);
        }
        if title == "Terminal" || title_is_cwd_echo {
            return Some(format!("Pane {}/{}", index + 1, pane_count));
        }
        Some(format!("Pane {}/{} · {}", index + 1, pane_count, title))
    });
    let mut signature = format!(
        "active={:?};status={:?};path={:?};pane={:?};rows={};",
        active_workspace_id,
        active_status_label,
        active_full_path,
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
        active_workspace_id: active_workspace_id.clone(),
        active_workspace_name,
        active_status_label,
        active_full_path,
        active_pane_label,
        signature,
    }
}

fn workspace_meta_line(workspace: &forktty_core::Workspace, ssh_host: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(host) = ssh_host {
        parts.push(format!("ssh:{host}"));
    }
    if !workspace.git_branch.trim().is_empty() {
        parts.push(workspace.git_branch.clone());
    }
    if let Some(worktree) = workspace.worktree_name.as_deref() {
        if !worktree.trim().is_empty() {
            parts.push(format!("wt:{worktree}"));
        }
    }
    if let Some(pr) = workspace.pr.as_ref() {
        parts.push(pr.summary());
    }
    parts.push(compact_path(&workspace.working_dir));
    if !workspace.listening_ports.is_empty() {
        let ports = workspace
            .listening_ports
            .iter()
            .map(|port| format!(":{port}"))
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(ports);
    }
    parts.join(" · ")
}

fn select_sidebar_workspace(
    state: &SocketAppState,
    workspace_id: &str,
    controller: &Rc<RefCell<VteController>>,
) {
    if let Err(err) = select_workspace_with_terminal(state, workspace_id) {
        eprintln!("Failed to spawn selected workspace terminal: {err}");
        create_global_notification(
            state,
            "Workspace Switch Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
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
    let Some(workspace) = model.active_workspace() else {
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
                tooltip: "Error reported in this workspace",
                class_name: "error",
            },
            NotificationKind::Prompt => WorkspaceStatusBadge {
                label: "Input",
                tooltip: "Needs input",
                class_name: "needs-input",
            },
            NotificationKind::Info | NotificationKind::Custom => WorkspaceStatusBadge {
                label: "Alert",
                tooltip: "Attention",
                class_name: "attention",
            },
        });
    }

    if workspace.needs_attention {
        return Some(WorkspaceStatusBadge {
            label: "Alert",
            tooltip: "Attention",
            class_name: "attention",
        });
    }

    if statuses.iter().any(status_entry_suggests_error) {
        return Some(WorkspaceStatusBadge {
            label: "Error",
            tooltip: "Error reported in this workspace",
            class_name: "error",
        });
    }

    if statuses.iter().any(status_entry_suggests_exited) {
        return Some(WorkspaceStatusBadge {
            label: "Exited",
            tooltip: "Process exited",
            class_name: "exited",
        });
    }

    if statuses.iter().any(status_entry_suggests_running) {
        return Some(WorkspaceStatusBadge {
            label: "Running",
            tooltip: "Process running",
            class_name: "running",
        });
    }

    if !progress.is_empty() {
        return Some(WorkspaceStatusBadge {
            label: "Working",
            tooltip: "Work in progress",
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
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        model.split_surface(&workspace.focused_surface_id, axis)
    };

    let Some(surface) = surface else {
        return;
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        if let Ok(mut model) = state.model.lock() {
            let _ = model.close_surface(&surface.id);
        }
        eprintln!("Failed to spawn split terminal: {err}");
        create_global_notification(
            state,
            "Split Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
    } else {
        save_session_from_state(state);
    }
}

#[cfg(feature = "browser")]
fn open_browser_active(state: &SocketAppState, axis: SplitAxis) {
    let opened = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to open browser pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        let workspace_id = workspace.id.clone();
        model.open_browser(
            &workspace_id,
            "about:blank",
            forktty_core::ProfileId::default(),
            axis,
        )
    };
    if opened.is_some() {
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
            .active_workspace()
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
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return false;
    }

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
            create_global_notification(
                state,
                "Restart Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return false;
        }
    }

    if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
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
    let (focused, root_replacement) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let focused = model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id);
        let Some(focused) = focused else {
            return;
        };
        if model.surface(&focused).is_none() {
            return;
        }
        let root_replacement = model.prepare_root_surface_replacement(&focused);
        (focused, root_replacement)
    };

    if let Some(replacement) = root_replacement {
        if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
            &replacement,
            state.shell.clone(),
            state.socket_path.clone(),
        )) {
            eprintln!("Failed to spawn replacement terminal surface: {err}");
            create_global_notification(
                state,
                "Close Pane Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return;
        }
        match state.terminal.close(&focused) {
            Ok(()) | Err(TerminalError::NotFound(_)) => {}
            Err(err) => {
                let mut message = err.to_string();
                if let Err(cleanup_err) = forget_terminal_surface_gtk(state, &replacement.id) {
                    message = format!("{message}; replacement cleanup failed: {cleanup_err}");
                }
                eprintln!("Failed to close terminal surface: {message}");
                create_global_notification(
                    state,
                    "Close Pane Failed",
                    &message,
                    NotificationKind::Error,
                );
                return;
            }
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let _ = model.close_surface_with_replacement(&focused, Some(replacement));
        }
        save_session_from_state(state);
        return;
    }

    match state.terminal.close(&focused) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to close terminal surface: {err}");
            create_global_notification(
                state,
                "Close Pane Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return;
        }
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
    let surface = {
        let model = state
            .model
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let Some(workspace) = model.active_workspace() else {
            return Ok(());
        };
        model.surface(&workspace.focused_surface_id).cloned()
    };
    let Some(surface) = surface else {
        return Ok(());
    };
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return Ok(());
    };
    if state
        .terminal
        .surfaces()?
        .iter()
        .any(|terminal_surface| terminal_surface.surface_id == surface.id)
    {
        return Ok(());
    }
    state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    ))
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

    let (active_id, workspaces, ssh_hosts) = {
        let Ok(model) = state.model.lock() else {
            return;
        };
        let workspaces = model.list_workspaces();
        let ssh_hosts: std::collections::BTreeMap<String, String> = workspaces
            .iter()
            .filter_map(|ws| {
                model.list_surfaces(Some(&ws.id)).into_iter().find_map(|s| {
                    if let forktty_core::SurfaceKind::Ssh { host } = s.kind {
                        Some((ws.id.clone(), host))
                    } else {
                        None
                    }
                })
            })
            .collect();
        (model.active_workspace_id(), workspaces, ssh_hosts)
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
            check.set_visible(is_active);
            inner.append(&check);

            let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
            body.set_hexpand(true);
            let name = gtk::Label::builder()
                .label(&ws.name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            name.add_css_class("ft-workspace-popover-name");
            let mut meta_parts: Vec<String> = Vec::new();
            if let Some(host) = ssh_hosts.get(&ws.id) {
                meta_parts.push(format!("ssh:{host}"));
            }
            let branch = ws.git_branch.trim();
            if !branch.is_empty() {
                meta_parts.push(branch.to_string());
            }
            if let Some(wt) = ws.worktree_name.as_deref() {
                let wt = wt.trim();
                if !wt.is_empty() {
                    meta_parts.push(format!("wt:{wt}"));
                }
            }
            meta_parts.push(compact_path(&ws.working_dir));
            let path = gtk::Label::builder()
                .label(meta_parts.join(" · "))
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
            ("Select All", "Ctrl+Shift+A"),
            ("Reset and Clear", "Command Palette / Context Menu"),
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

fn show_about_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::AboutDialog::builder()
        .transient_for(parent)
        .modal(true)
        .program_name("ForkTTY")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Native GTK/VTE workspace terminal for panes, worktrees and socket automation.")
        .website("https://github.com/Lucenx9/forktty")
        .website_label("GitHub Repository")
        .logo_icon_name("forktty")
        .build();
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
            let body = close_workspace_confirmation_body(&workspace.name, &workspace.working_dir);
            show_destructive_confirmation(
                &parent,
                "Close Workspace?",
                &body,
                "Close Workspace",
                move || close_active_workspace(&state_confirm),
            );
        }
    });
    command!("About ForkTTY", None, {
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            show_about_dialog(&parent);
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

#[derive(Clone, Debug)]
struct WorktreeDialogChoice {
    selector: String,
    label: String,
}

fn worktree_dialog_choices(state: &SocketAppState) -> Vec<WorktreeDialogChoice> {
    let Ok(cwd) = active_workspace_cwd_string(state) else {
        return Vec::new();
    };
    let Ok(mut worktrees) = worktree::list(&cwd) else {
        return Vec::new();
    };
    worktrees.sort_by(|left, right| {
        left.worktree_name
            .cmp(&right.worktree_name)
            .then(left.branch.cmp(&right.branch))
    });
    worktrees
        .into_iter()
        .map(|info| {
            let path = compact_path(Path::new(&info.path));
            let label = if info.branch == info.worktree_name {
                format!("{} · {path}", info.worktree_name)
            } else {
                format!("{} · {} · {path}", info.worktree_name, info.branch)
            };
            WorktreeDialogChoice {
                selector: info.worktree_name,
                label,
            }
        })
        .collect()
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
            model.active_workspace().map(|workspace| {
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
    let existing_worktrees = worktree_dialog_choices(state);
    let existing = gtk::ComboBoxText::new();
    existing.add_css_class("worktree-existing");
    existing.set_tooltip_text(Some("Existing worktree to merge or remove"));
    existing.update_property(&[gtk::accessible::Property::Label("Existing worktree")]);
    for choice in &existing_worktrees {
        existing.append(Some(&choice.selector), &choice.label);
    }
    if !existing_worktrees.is_empty() {
        existing.set_active(Some(0));
    }
    existing.set_visible(false);
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
    body.append(&existing);
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
        title: title.clone(),
        subtitle: subtitle.clone(),
        entry: entry.clone(),
        existing: existing.clone(),
        has_existing_worktrees: !existing_worktrees.is_empty(),
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
    existing.connect_changed({
        let entry = entry.clone();
        let refresh = refresh.clone();
        move |combo| {
            if let Some(selector) = combo.active_id() {
                entry.set_text(selector.as_str());
            }
            refresh(true);
        }
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
                        "Remove worktree '{name}' and close its ForkTTY workspace. The git branch is left intact."
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
    title: gtk::Label,
    subtitle: gtk::Label,
    entry: gtk::Entry,
    existing: gtk::ComboBoxText,
    has_existing_worktrees: bool,
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
    fn dialog_title(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create Worktree",
            WorktreeDialogMode::Attach => "Attach Worktree",
            WorktreeDialogMode::Merge => "Merge Worktree",
            WorktreeDialogMode::Remove => "Remove Worktree",
        }
    }

    fn dialog_subtitle(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create a new isolated git worktree workspace.",
            WorktreeDialogMode::Attach => "Open an existing branch or linked worktree.",
            WorktreeDialogMode::Merge => {
                "Choose an existing worktree to merge into the base checkout."
            }
            WorktreeDialogMode::Remove => {
                "Choose an existing worktree to remove after dirty-state checks."
            }
        }
    }

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

    fn uses_existing_chooser(self) -> bool {
        matches!(self, WorktreeDialogMode::Merge | WorktreeDialogMode::Remove)
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
    controls.title.set_label(mode.dialog_title());
    controls.subtitle.set_label(mode.dialog_subtitle());
    controls
        .entry
        .set_placeholder_text(Some(mode.placeholder()));
    controls.entry.set_tooltip_text(Some(mode.tooltip()));
    let use_existing_chooser = mode.uses_existing_chooser() && controls.has_existing_worktrees;
    controls.entry.set_visible(!use_existing_chooser);
    controls.existing.set_visible(use_existing_chooser);
    if use_existing_chooser {
        if let Some(selector) = controls.existing.active_id() {
            if controls.entry.text().as_str() != selector.as_str() {
                controls.entry.set_text(selector.as_str());
            }
        }
    }
    controls.hint.set_label(
        if mode.uses_existing_chooser() && !controls.has_existing_worktrees {
            "No linked worktrees were found for this repository. Type a worktree or branch name manually."
        } else {
            mode.hint()
        },
    );
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

    let name = if use_existing_chooser {
        controls
            .existing
            .active_id()
            .map(|selector| selector.to_string())
            .unwrap_or_default()
    } else {
        controls.entry.text().to_string()
    };
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
    let cwd = active_workspace_cwd(state).ok_or_else(no_active_workspace_message)?;
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
        .spawn(SpawnRequest::for_workspace(
            &workspace,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
        .map_err(|err| err.to_string())
    {
        let mut err = err;
        if let Err(rollback_err) =
            rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id)
        {
            err = format!("{err}; workspace rollback failed: {rollback_err}");
        }
        if matches!(action, WorktreeAction::Create) {
            return Err(rollback_created_worktree_after_spawn_failure(
                &cwd, &info, err,
            ));
        }
        return Err(err);
    }
    save_session_from_state(state);
    Ok(())
}

fn rollback_created_worktree_after_spawn_failure(
    cwd: &str,
    info: &worktree::WorktreeInfo,
    spawn_error: String,
) -> String {
    match worktree::remove(cwd, &info.worktree_name, true) {
        Ok(()) => spawn_error,
        Err(rollback_error) => format!(
            "{spawn_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
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
    worktree::remove(&cwd, name, false).map_err(|err| err.to_string())?;
    close_workspace_by_worktree_name(state, &workspace_worktree_name, fallback_path)
        .map_err(|err| err.to_string())?;
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
    validate_worktree_name(name).map_err(|err| match err {
        WorktreeNameError::Empty => "Branch or worktree name is required".to_string(),
        WorktreeNameError::TooLong => {
            "Branch or worktree name must be 255 bytes or fewer".to_string()
        }
        WorktreeNameError::UnsupportedCharacters => {
            "Branch or worktree name contains unsupported characters".to_string()
        }
        WorktreeNameError::UnsafeSegment => {
            "Branch or worktree name contains an unsafe path segment".to_string()
        }
    })
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
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(no_active_workspace_message)
}

fn no_active_workspace_message() -> String {
    "No active workspace is available for worktree operations.".to_string()
}

fn close_workspace_by_worktree_name(
    state: &SocketAppState,
    worktree_name: &str,
    fallback_path: PathBuf,
) -> Result<(), TerminalError> {
    let (workspace, surface_ids, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned),
        };
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(worktree_name));
        let Some(workspace) = workspace else {
            return Ok(());
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace, surface_ids, is_last_workspace)
    };
    if is_last_workspace {
        let (replacement, previous_active_id) = {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned),
            };
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", fallback_path.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal_gtk(state, &replacement) {
            let mut err = err;
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; workspace rollback failed: {rollback_err}"
                ));
            }
            return Err(err);
        }
        if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
            let mut err = err;
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; replacement cleanup failed: {cleanup_err}"
                ));
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; workspace rollback failed: {rollback_err}"
                ));
            }
            return Err(err);
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned),
            };
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        return Ok(());
    }
    close_terminal_surfaces(state, &surface_ids)?;
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned),
        };
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
    }
    Ok(())
}

fn active_workspace_cwd(state: &SocketAppState) -> Option<PathBuf> {
    state.model.lock().ok().and_then(|model| {
        model
            .active_workspace()
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

fn latest_openable_notification(state: &SocketAppState) -> Option<NotificationItem> {
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    notifications
        .into_iter()
        .rev()
        .find(|notification| notification_target_exists(state, notification))
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
        create_global_notification(
            state,
            "Open Notification Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
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
        .map(|mut model| {
            let notifications = model.list_notifications();
            model.mark_notifications_read();
            notifications
        })
        .unwrap_or_default();
    let has_notifications = !notifications.is_empty();
    let subtitle = gtk::Label::builder()
        .label(if has_notifications {
            format!(
                "{} {}",
                notifications.len(),
                if notifications.len() == 1 {
                    "notification"
                } else {
                    "notifications"
                }
            )
        } else {
            "No notifications".to_string()
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

    let close_button = gtk::Button::with_label("Close");
    let jump = gtk::Button::with_label("Open Latest");
    jump.set_sensitive(latest_openable_notification(state).is_some());
    jump.set_tooltip_text(Some("Open the latest notification with a workspace target"));
    let clear = gtk::Button::with_label("Clear All");
    clear.set_sensitive(has_notifications);
    clear.add_css_class("destructive-action");
    clear.set_tooltip_text(Some("Clear pending notifications"));

    let show_empty_state = {
        let body = body.clone();
        let subtitle = subtitle.clone();
        let clear = clear.clone();
        let jump = jump.clone();
        Rc::new(move || {
            while let Some(child) = body.first_child() {
                body.remove(&child);
            }
            let empty = compact_status_page(
                "preferences-system-notifications-symbolic",
                "All Clear",
                "Prompts and alerts will appear here.",
            );
            body.append(&empty);
            subtitle.set_label("No notifications");
            clear.set_sensitive(false);
            jump.set_sensitive(false);
        })
    };

    let refresh_jump_state = {
        let state = state.clone();
        let jump = jump.clone();
        Rc::new(move || {
            jump.set_sensitive(latest_openable_notification(&state).is_some());
        })
    };

    if !has_notifications {
        show_empty_state();
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
            if !notification.read {
                card.add_css_class("unread");
            }

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
            let dismiss = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Dismiss notification")
                .build();
            dismiss.add_css_class("flat");
            dismiss.add_css_class("notification-dismiss");
            set_accessible_button_text(&dismiss, "Dismiss notification", None);
            let state_for_dismiss = state.clone();
            let notification_id = notification.id.clone();
            let row_for_dismiss = row.clone();
            let subtitle_for_dismiss = subtitle.clone();
            let show_empty_for_dismiss = show_empty_state.clone();
            let refresh_jump_for_dismiss = refresh_jump_state.clone();
            dismiss.connect_clicked(move |_| {
                let remaining = state_for_dismiss
                    .model
                    .lock()
                    .ok()
                    .map(|mut model| {
                        model.dismiss_notification(&notification_id);
                        model.list_notifications().len()
                    })
                    .unwrap_or(0);
                row_for_dismiss.set_visible(false);
                if remaining == 0 {
                    show_empty_for_dismiss();
                } else {
                    let label = if remaining == 1 {
                        "1 notification".to_string()
                    } else {
                        format!("{remaining} notifications")
                    };
                    subtitle_for_dismiss.set_label(&label);
                    refresh_jump_for_dismiss();
                }
            });
            top.append(&dismiss);

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
    footer.append(&spacer);
    footer.append(&close_button);
    footer.append(&jump);
    footer.append(&clear);

    let dialog_for_close = dialog.clone();
    close_button.connect_clicked(move |_| dialog_for_close.close());

    {
        let state_for_jump = state.clone();
        let controller_for_jump = controller.clone();
        let dialog_for_jump = dialog.clone();
        let jump_for_click = jump.clone();
        jump.connect_clicked(move |_| {
            let Some(notification) = latest_openable_notification(&state_for_jump) else {
                jump_for_click.set_sensitive(false);
                return;
            };
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
    let show_empty_for_clear = show_empty_state.clone();
    clear.connect_clicked(move |_| {
        if let Ok(mut model) = state_for_clear.model.lock() {
            model.clear_notifications();
        }
        show_empty_for_clear();
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
    let dialog = adw::PreferencesWindow::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(840)
        .default_height(680)
        .search_enabled(true)
        .build();
    let loaded = config::load_config().unwrap_or_default();
    let current = Rc::new(RefCell::new(loaded.clone()));
    let suppress_updates = Rc::new(Cell::new(false));

    install_escape_close(dialog.upcast_ref::<gtk::Window>());
    restore_focus_after_hide(dialog.upcast_ref::<gtk::Window>(), parent);

    let terminal_page = adw::PreferencesPage::builder()
        .title("Terminal")
        .icon_name("utilities-terminal-symbolic")
        .build();
    let shell_group = adw::PreferencesGroup::builder()
        .title("Shell")
        .description("Controls how new terminal sessions are started.")
        .build();
    let shell_entry = adw::EntryRow::builder()
        .title("Shell command")
        .text(&loaded.general.shell)
        .show_apply_button(true)
        .tooltip_text("Absolute path to the shell executable")
        .build();
    shell_group.add(&shell_entry);
    terminal_page.add(&shell_group);

    let font_group = adw::PreferencesGroup::builder()
        .title("Text")
        .description("Applied immediately to all open VTE panes.")
        .build();
    let font_family = font_family_combo(parent, &loaded.appearance.font_family);
    font_family.set_tooltip_text(Some("Terminal font family"));
    font_family.set_hexpand(false);
    font_family.set_halign(gtk::Align::End);
    font_family.set_valign(gtk::Align::Center);
    font_family.set_width_request(220);
    for cell in font_family.cells() {
        if let Ok(text) = cell.downcast::<gtk::CellRendererText>() {
            text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.set_max_width_chars(28);
        }
    }
    let font_family_row =
        settings_action_row("Font family", "Monospace font with symbol coverage.");
    font_family_row.add_suffix(&font_family);
    font_family_row.set_activatable_widget(Some(&font_family));
    font_group.add(&font_family_row);

    let (font_size_row, font_size) = settings_number_row(
        "Font size",
        "Terminal text size, in points.",
        8,
        64,
        1,
        i64::from(loaded.appearance.font_size),
        96,
    );
    font_group.add(&font_size_row);
    terminal_page.add(&font_group);

    let behavior_group = adw::PreferencesGroup::builder()
        .title("Behavior")
        .description("Runtime behavior for VTE panes.")
        .build();
    let (scrollback_lines_row, scrollback_lines) = settings_number_row(
        "Scrollback lines",
        "Set to 0 to disable saved scrollback for each pane.",
        0,
        500_000,
        1000,
        i64::from(loaded.appearance.scrollback_lines),
        128,
    );
    behavior_group.add(&scrollback_lines_row);
    let terminal_audible_bell = adw::SwitchRow::builder()
        .title("Audible bell")
        .subtitle("Let terminal bell sequences play the system alert sound.")
        .active(loaded.appearance.terminal_audible_bell)
        .build();
    behavior_group.add(&terminal_audible_bell);
    terminal_page.add(&behavior_group);
    dialog.add(&terminal_page);

    let appearance_page = adw::PreferencesPage::builder()
        .title("Interface")
        .icon_name("preferences-desktop-theme-symbolic")
        .build();
    let theme_group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("Controls the GTK chrome and VTE palette.")
        .build();
    let (theme_source_row, theme_source) = settings_combo_row(
        "Color scheme",
        "Use the system preference or force a light/dark app theme.",
        &[("auto", "System"), ("light", "Light"), ("dark", "Dark")],
        &loaded.general.theme_source,
    );
    theme_group.add(&theme_source_row);
    let (terminal_theme_row, terminal_theme) = settings_combo_row(
        "Terminal theme",
        "System follows the app color scheme; named themes use fixed dark palettes.",
        TERMINAL_THEME_ITEMS,
        &loaded.appearance.terminal_theme,
    );
    theme_group.add(&terminal_theme_row);
    appearance_page.add(&theme_group);

    let window_group = adw::PreferencesGroup::builder()
        .title("Window")
        .description("Window layout and workspace sidebar behavior.")
        .build();
    let (window_mode_row, window_mode) = settings_combo_row(
        "Window mode",
        "Quake mode uses a drop-down window after restart.",
        &[("normal", "Normal"), ("quake", "Quake")],
        &loaded.appearance.window_mode,
    );
    window_group.add(&window_mode_row);
    let (sidebar_position_row, sidebar_position) = settings_combo_row(
        "Sidebar position",
        "Side of the main window used for workspaces.",
        &[("left", "Left"), ("right", "Right")],
        &loaded.appearance.sidebar_position,
    );
    window_group.add(&sidebar_position_row);
    let sidebar_visible = adw::SwitchRow::builder()
        .title("Show sidebar on startup")
        .subtitle("You can still toggle it with Ctrl+B or F9.")
        .active(loaded.appearance.sidebar_visible)
        .build();
    window_group.add(&sidebar_visible);
    let show_workspace_tabs = adw::SwitchRow::builder()
        .title("Show workspace tabs")
        .subtitle("Horizontal tab bar below the titlebar showing all workspaces.")
        .active(loaded.appearance.show_workspace_tabs)
        .build();
    window_group.add(&show_workspace_tabs);
    appearance_page.add(&window_group);
    dialog.add(&appearance_page);

    let automation_page = adw::PreferencesPage::builder()
        .title("Automation")
        .icon_name("system-run-symbolic")
        .build();
    let worktree_group = adw::PreferencesGroup::builder()
        .title("Git Worktrees")
        .description("Controls where new worktree directories are created.")
        .build();
    let (worktree_layout_row, worktree_layout) = settings_combo_row(
        "Worktree layout",
        "Placement for new worktree directories relative to the repository root.",
        &[
            ("nested", "Nested"),
            ("sibling", "Sibling"),
            ("outer-nested", "Outer nested"),
        ],
        &loaded.general.worktree_layout,
    );
    worktree_group.add(&worktree_layout_row);
    let pr_lookup = adw::SwitchRow::builder()
        .title("Linked PR lookup")
        .subtitle("Use the GitHub CLI to show PR status for workspace branches.")
        .active(loaded.general.enable_pr_lookup)
        .build();
    worktree_group.add(&pr_lookup);
    automation_page.add(&worktree_group);

    let notification_group = adw::PreferencesGroup::builder()
        .title("Notifications")
        .description("In-app notifications always remain available in the notification panel.")
        .build();
    let notification_command = adw::EntryRow::builder()
        .title("Custom command")
        .text(&loaded.general.notification_command)
        .show_apply_button(true)
        .tooltip_text("Optional absolute command to run when a notification fires")
        .build();
    notification_command.set_input_purpose(gtk::InputPurpose::Terminal);
    notification_group.add(&notification_command);
    let desktop_notifications = adw::SwitchRow::builder()
        .title("Desktop notifications")
        .subtitle("Forward alerts to the system notification daemon.")
        .active(loaded.notifications.desktop)
        .build();
    notification_group.add(&desktop_notifications);
    let notification_sound = adw::SwitchRow::builder()
        .title("Alert sound")
        .subtitle("Play the default system alert sound for ForkTTY alerts.")
        .active(loaded.notifications.sound)
        .build();
    notification_group.add(&notification_sound);
    automation_page.add(&notification_group);

    let maintenance_group = adw::PreferencesGroup::builder()
        .title("Advanced")
        .description("Preferences are saved to the user config file immediately.")
        .build();
    let reset_row = settings_action_row(
        "Reset to defaults",
        "Restore the default shell, appearance, workspace and notification preferences.",
    );
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("destructive-action");
    reset_row.add_suffix(&reset);
    reset_row.set_activatable_widget(Some(&reset));
    maintenance_group.add(&reset_row);
    automation_page.add(&maintenance_group);
    dialog.add(&automation_page);

    shell_entry.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let mut next = current.borrow().clone();
            next.general.shell = row.text().to_string();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Shell saved. Restart ForkTTY to use it.",
            );
        }
    });
    font_family.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(family) = combo.active_id() {
                let family = family.to_string();
                let mut next = current.borrow().clone();
                next.appearance.font_family = match family.as_str() {
                    DEFAULT_FONT_FAMILY_ID => String::new(),
                    SYSTEM_MONOSPACE_FONT_FAMILY_ID => "monospace".to_string(),
                    _ => decode_font_family_row_id(&family),
                };
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Terminal font applied.",
                );
            }
        }
    });
    connect_settings_number_control(&font_size, {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |value| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.font_size = value as u16;
            persist_settings_change(&dialog, &current, &on_apply, next, "Font size applied.");
        }
    });
    connect_settings_number_control(&scrollback_lines, {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |value| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.scrollback_lines = value as u32;
            persist_settings_change(&dialog, &current, &on_apply, next, "Scrollback updated.");
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
            let mut next = current.borrow().clone();
            next.appearance.terminal_audible_bell = row.is_active();
            persist_settings_change(&dialog, &current, &on_apply, next, "Terminal bell updated.");
        }
    });
    theme_source.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(theme) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.general.theme_source = theme.to_string();
                persist_settings_change(&dialog, &current, &on_apply, next, "Theme applied.");
            }
        }
    });
    terminal_theme.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(theme) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.terminal_theme = theme.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Terminal theme applied.",
                );
            }
        }
    });
    window_mode.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(mode) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.window_mode = mode.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Window mode saved. Restart ForkTTY to use it.",
                );
            }
        }
    });
    sidebar_position.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(position) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.sidebar_position = position.to_string();
                persist_settings_change(&dialog, &current, &on_apply, next, "Sidebar moved.");
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
            let mut next = current.borrow().clone();
            next.appearance.sidebar_visible = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Sidebar visibility updated.",
            );
        }
    });
    show_workspace_tabs.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.show_workspace_tabs = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Workspace tabs visibility updated.",
            );
        }
    });
    worktree_layout.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(layout) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.general.worktree_layout = layout.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
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
            let mut next = current.borrow().clone();
            next.general.enable_pr_lookup = row.is_active();
            persist_settings_change(&dialog, &current, &on_apply, next, "PR lookup updated.");
        }
    });
    notification_command.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let mut next = current.borrow().clone();
            next.general.notification_command = row.text().to_string();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Notification command saved.",
            );
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
            let mut next = current.borrow().clone();
            next.notifications.desktop = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
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
            let mut next = current.borrow().clone();
            next.notifications.sound = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Notification sound updated.",
            );
        }
    });
    reset.connect_clicked({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |_| {
            let confirmation_parent = dialog.clone();
            let dialog_for_reset = dialog.clone();
            let current_for_reset = current.clone();
            let on_apply_for_reset = on_apply.clone();
            let suppress_updates_for_reset = suppress_updates.clone();
            let shell_entry_for_reset = shell_entry.clone();
            let font_family_for_reset = font_family.clone();
            let font_size_for_reset = font_size.clone();
            let scrollback_lines_for_reset = scrollback_lines.clone();
            let terminal_audible_bell_for_reset = terminal_audible_bell.clone();
            let theme_source_for_reset = theme_source.clone();
            let terminal_theme_for_reset = terminal_theme.clone();
            let window_mode_for_reset = window_mode.clone();
            let sidebar_position_for_reset = sidebar_position.clone();
            let sidebar_visible_for_reset = sidebar_visible.clone();
            let show_workspace_tabs_for_reset = show_workspace_tabs.clone();
            let worktree_layout_for_reset = worktree_layout.clone();
            let pr_lookup_for_reset = pr_lookup.clone();
            let notification_command_for_reset = notification_command.clone();
            let desktop_notifications_for_reset = desktop_notifications.clone();
            let notification_sound_for_reset = notification_sound.clone();
            show_destructive_confirmation(
                &confirmation_parent,
                "Reset Settings?",
                "Restore ForkTTY settings to their default values. This changes the saved shell, appearance, workspace, and notification preferences.",
                "Reset Settings",
                move || {
                    let defaults = config::AppConfig::default();
                    suppress_updates_for_reset.set(true);
                    shell_entry_for_reset.set_text(&defaults.general.shell);
                    let _ = font_family_for_reset.set_active_id(Some(DEFAULT_FONT_FAMILY_ID));
                    font_size_for_reset.set_value(i64::from(defaults.appearance.font_size));
                    scrollback_lines_for_reset
                        .set_value(i64::from(defaults.appearance.scrollback_lines));
                    terminal_audible_bell_for_reset
                        .set_active(defaults.appearance.terminal_audible_bell);
                    let _ =
                        theme_source_for_reset.set_active_id(Some(&defaults.general.theme_source));
                    let _ = terminal_theme_for_reset
                        .set_active_id(Some(&defaults.appearance.terminal_theme));
                    let _ =
                        window_mode_for_reset.set_active_id(Some(&defaults.appearance.window_mode));
                    let _ = sidebar_position_for_reset
                        .set_active_id(Some(&defaults.appearance.sidebar_position));
                    sidebar_visible_for_reset.set_active(defaults.appearance.sidebar_visible);
                    show_workspace_tabs_for_reset
                        .set_active(defaults.appearance.show_workspace_tabs);
                    let _ =
                        worktree_layout_for_reset.set_active_id(Some(&defaults.general.worktree_layout));
                    pr_lookup_for_reset.set_active(defaults.general.enable_pr_lookup);
                    notification_command_for_reset.set_text(&defaults.general.notification_command);
                    desktop_notifications_for_reset.set_active(defaults.notifications.desktop);
                    notification_sound_for_reset.set_active(defaults.notifications.sound);
                    suppress_updates_for_reset.set(false);
                    persist_settings_change(
                        &dialog_for_reset,
                        &current_for_reset,
                        &on_apply_for_reset,
                        defaults,
                        "Defaults restored.",
                    );
                },
            );
        }
    });

    dialog.present();
}

fn settings_action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_lines(0)
        .build()
}

fn settings_combo_row(
    title: &str,
    subtitle: &str,
    items: &[(&str, &str)],
    active_id: &str,
) -> (adw::ActionRow, gtk::ComboBoxText) {
    let row = settings_action_row(title, subtitle);
    let combo = combo_with_ids(items, active_id);
    combo.set_valign(gtk::Align::Center);
    combo.set_width_request(180);
    row.add_suffix(&combo);
    row.set_activatable_widget(Some(&combo));
    (row, combo)
}

#[derive(Clone)]
struct SettingsNumberControl {
    entry: gtk::Entry,
    decrement: gtk::Button,
    increment: gtk::Button,
    min: i64,
    max: i64,
    step: i64,
}

impl SettingsNumberControl {
    fn value(&self) -> i64 {
        self.entry
            .text()
            .trim()
            .parse::<i64>()
            .unwrap_or(self.min)
            .clamp(self.min, self.max)
    }

    fn set_value(&self, value: i64) {
        self.entry
            .set_text(&value.clamp(self.min, self.max).to_string());
    }

    fn stepped_value(&self, delta: i64) -> i64 {
        self.value().saturating_add(delta).clamp(self.min, self.max)
    }
}

fn settings_number_row(
    title: &str,
    subtitle: &str,
    min: i64,
    max: i64,
    step: i64,
    value: i64,
    width: i32,
) -> (adw::ActionRow, SettingsNumberControl) {
    let row = settings_action_row(title, subtitle);
    let control = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    control.add_css_class("settings-number-control");
    control.set_valign(gtk::Align::Center);
    control.set_halign(gtk::Align::End);

    let entry = gtk::Entry::builder()
        .text(value.clamp(min, max).to_string())
        .width_request(width)
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    entry.add_css_class("settings-number-entry");
    gtk::prelude::EntryExt::set_alignment(&entry, 1.0);

    let decrement = gtk::Button::with_label("-");
    decrement.add_css_class("settings-number-button");
    decrement.set_tooltip_text(Some("Decrease"));
    set_accessible_button_text(&decrement, "Decrease", None);

    let increment = gtk::Button::with_label("+");
    increment.add_css_class("settings-number-button");
    increment.set_tooltip_text(Some("Increase"));
    set_accessible_button_text(&increment, "Increase", None);

    control.append(&decrement);
    control.append(&entry);
    control.append(&increment);
    row.add_suffix(&control);
    row.set_activatable_widget(Some(&entry));

    (
        row,
        SettingsNumberControl {
            entry,
            decrement,
            increment,
            min,
            max,
            step,
        },
    )
}

fn connect_settings_number_control<F>(control: &SettingsNumberControl, apply: F)
where
    F: Fn(i64) + 'static,
{
    let apply: Rc<dyn Fn(i64)> = Rc::new(apply);

    control.decrement.connect_clicked({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.stepped_value(-control.step);
            control.set_value(value);
            apply(value);
        }
    });

    control.increment.connect_clicked({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.stepped_value(control.step);
            control.set_value(value);
            apply(value);
        }
    });

    control.entry.connect_activate({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.value();
            control.set_value(value);
            apply(value);
        }
    });

    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave({
        let control = control.clone();
        move |_| {
            let value = control.value();
            control.set_value(value);
            apply(value);
        }
    });
    control.entry.add_controller(focus);
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

fn persist_settings_change(
    dialog: &adw::PreferencesWindow,
    current: &Rc<RefCell<config::AppConfig>>,
    on_apply: &SettingsApplyCallback,
    next: config::AppConfig,
    message: &str,
) -> bool {
    if *current.borrow() == next {
        return true;
    }
    match config::save_config(&next) {
        Ok(()) => {
            *current.borrow_mut() = next.clone();
            on_apply(&next);
            dialog.add_toast(adw::Toast::new(message));
            true
        }
        Err(err) => {
            dialog.add_toast(adw::Toast::new(&err.to_string()));
            false
        }
    }
}

fn installed_font_family_row_id(name: &str) -> String {
    format!("{INSTALLED_FONT_FAMILY_ID_PREFIX}{name}")
}

fn decode_font_family_row_id(id: &str) -> String {
    id.strip_prefix(INSTALLED_FONT_FAMILY_ID_PREFIX)
        .unwrap_or(id)
        .to_string()
}

fn font_family_combo(parent: &impl IsA<gtk::Widget>, active_family: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    let active_family = active_family.trim();
    let all_names = installed_font_families(parent);
    let default_family = default_terminal_font_family(&all_names);
    combo.append(
        Some(DEFAULT_FONT_FAMILY_ID),
        &format!("Default terminal font ({default_family})"),
    );
    let system_monospace =
        resolved_system_monospace_family(parent).unwrap_or_else(|| "monospace".to_string());
    combo.append(
        Some(SYSTEM_MONOSPACE_FONT_FAMILY_ID),
        &format!("System monospace ({system_monospace})"),
    );

    let mut names = installed_monospace_font_families(parent);
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
        combo.append(Some(&installed_font_family_row_id(name)), name);
    }
    if !has_active {
        combo.append(
            Some(&installed_font_family_row_id(active_family)),
            &format!("{active_family} (saved)"),
        );
    }

    let active_id = if active_family.is_empty() {
        DEFAULT_FONT_FAMILY_ID.to_string()
    } else if active_family.eq_ignore_ascii_case("monospace") {
        SYSTEM_MONOSPACE_FONT_FAMILY_ID.to_string()
    } else {
        installed_font_family_row_id(active_family)
    };
    if !combo.set_active_id(Some(&active_id)) {
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
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_workspace(
        &workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
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
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_workspace(
        &workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        let _ = rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id);
        eprintln!("Failed to create workspace terminal: {err}");
        create_global_notification(
            state,
            "Workspace Create Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
    } else {
        save_session_from_state(state);
    }
}

fn rename_workspace_gtk(
    state: &SocketAppState,
    workspace_id: &str,
    new_name: &str,
) -> Result<(), String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("Workspace name cannot be empty.".to_string());
    }
    if trimmed.chars().count() > 80 {
        return Err("Workspace name must be 80 characters or fewer.".to_string());
    }
    {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if model
            .list_workspaces()
            .into_iter()
            .any(|workspace| workspace.id != workspace_id && workspace.name == trimmed)
        {
            return Err(format!("A workspace named '{trimmed}' already exists."));
        }
        model
            .rename_workspace(WorkspaceSelector::Id(workspace_id), trimmed)
            .ok_or_else(|| "Workspace no longer exists.".to_string())?;
    }
    save_session_from_state(state);
    Ok(())
}

fn close_active_workspace(state: &SocketAppState) {
    let (workspace, surface_ids, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace, surface_ids, is_last_workspace)
    };

    if is_last_workspace {
        let (replacement, previous_active_id) = {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", workspace.working_dir.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal_gtk(state, &replacement) {
            let mut message = err.to_string();
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                message = format!("{message}; workspace rollback failed: {rollback_err}");
            }
            notify_close_workspace_failed(state, &message);
            return;
        }
        if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
            let mut message = err.to_string();
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                message = format!("{message}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                message = format!("{message}; workspace rollback failed: {rollback_err}");
            }
            notify_close_workspace_failed(state, &message);
            return;
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        save_session_from_state(state);
        return;
    }

    if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
        notify_close_workspace_failed(state, &err.to_string());
        return;
    }

    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
    }
    save_session_from_state(state);
}

fn spawn_workspace_terminal_gtk(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), TerminalError> {
    state.terminal.spawn(SpawnRequest::for_workspace(
        workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    ))
}

fn close_terminal_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), TerminalError> {
    for surface_id in surface_ids {
        match state.terminal.close(surface_id) {
            Ok(()) | Err(TerminalError::NotFound(_)) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn forget_terminal_surface_gtk(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), TerminalError> {
    match state.terminal.forget_surface(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err),
    }
}

fn notify_close_workspace_failed(state: &SocketAppState, message: &str) {
    eprintln!("Failed to close workspace: {message}");
    create_global_notification(
        state,
        "Close Workspace Failed",
        message,
        NotificationKind::Error,
    );
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
            .active_workspace()
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

#[cfg(feature = "browser")]
fn handle_browser_command(
    controller: &Rc<RefCell<VteController>>,
    cmd: forktty_core::BrowserCommand,
) {
    use crate::browser_pane::{click_js, fill_js};
    use forktty_core::{BrowserCmdError, BrowserOp, CmdResult};

    let pane = controller.borrow().browser_pane(&cmd.surface_id);
    let Some(pane) = pane else {
        let _ = cmd.reply.send(CmdResult::Err(BrowserCmdError::NoWebView));
        return;
    };
    let reply = cmd.reply;
    match cmd.op {
        BrowserOp::Snapshot => {
            pane.run_js("window.__forktty.snapshot()", move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Click { reference } => {
            pane.run_js(&click_js(&reference), move |r| {
                let _ = reply.send(into_ok_cmd_result(r));
            });
        }
        BrowserOp::Fill { reference, value } => {
            pane.run_js(&fill_js(&reference, &value), move |r| {
                let _ = reply.send(into_ok_cmd_result(r));
            });
        }
        BrowserOp::Eval { script } => {
            pane.run_js(&script, move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        // Nav ops are fire-and-forget: Ok means "navigation initiated", not
        // "page loaded". Callers issue a follow-up snapshot to see the result.
        BrowserOp::Back => {
            pane.go_back();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Forward => {
            pane.go_forward();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Reload => {
            pane.reload();
            let _ = reply.send(CmdResult::Ok);
        }
    }
}

#[cfg(feature = "browser")]
fn into_cmd_result(r: Result<String, forktty_core::BrowserCmdError>) -> forktty_core::CmdResult {
    match r {
        Ok(json) => forktty_core::CmdResult::Json(json),
        Err(e) => forktty_core::CmdResult::Err(e),
    }
}

#[cfg(feature = "browser")]
fn into_ok_cmd_result(r: Result<String, forktty_core::BrowserCmdError>) -> forktty_core::CmdResult {
    match r {
        Ok(_) => forktty_core::CmdResult::Ok,
        Err(e) => forktty_core::CmdResult::Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Repository;

    fn make_temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("note.txt"), "base\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("note.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        drop(repo);
        dir
    }

    fn test_spawn_request() -> SpawnRequest {
        SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        }
    }

    #[derive(Debug, Default)]
    struct SecondSpawnFailsBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
        spawn_count: Mutex<usize>,
    }

    impl TerminalBackend for SecondSpawnFailsBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            let mut spawn_count = self
                .spawn_count
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            *spawn_count += 1;
            if *spawn_count > 1 {
                return Err(TerminalError::Backend("spawn failed".to_string()));
            }
            drop(spawn_count);
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(
                    request.surface_id.clone(),
                    TerminalSurfaceState {
                        surface_id: request.surface_id,
                        workspace_id: request.workspace_id,
                        cwd: request.cwd,
                        shell: request.shell,
                        cols: 80,
                        rows: 24,
                    },
                );
            Ok(())
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
    }

    #[derive(Debug, Default)]
    struct CloseFailsBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    }

    impl TerminalBackend for CloseFailsBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(
                    request.surface_id.clone(),
                    TerminalSurfaceState {
                        surface_id: request.surface_id,
                        workspace_id: request.workspace_id,
                        cwd: request.cwd,
                        shell: request.shell,
                        cols: 80,
                        rows: 24,
                    },
                );
            Ok(())
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, _surface_id: &str) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("close failed".to_string()))
        }

        fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
    }

    #[test]
    fn gtk_backend_rolls_back_spawn_when_ui_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let backend = GtkVteBackend::new(tx);

        let err = backend.spawn(test_spawn_request()).unwrap_err();

        assert!(matches!(err, TerminalError::Backend(_)));
        assert!(backend.surfaces().unwrap().is_empty());
    }

    #[test]
    fn gtk_backend_rolls_back_resize_when_ui_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        let backend = GtkVteBackend::new(tx);
        backend.spawn(test_spawn_request()).unwrap();
        drop(rx);

        let err = backend.resize("surface-1", 120, 40).unwrap_err();

        assert!(matches!(err, TerminalError::Backend(_)));
        let mut surfaces = backend.surfaces().unwrap();
        let surface = surfaces.remove(0);
        assert_eq!((surface.cols, surface.rows), (80, 24));
    }

    #[test]
    fn gtk_backend_rolls_back_close_when_ui_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        let backend = GtkVteBackend::new(tx);
        backend.spawn(test_spawn_request()).unwrap();
        drop(rx);

        let err = backend.close("surface-1").unwrap_err();

        assert!(matches!(err, TerminalError::Backend(_)));
        assert_eq!(backend.surfaces().unwrap().len(), 1);
    }

    #[test]
    fn child_exit_pid_removal_ignores_stale_spawn_tokens() {
        let mut pids = BTreeMap::new();
        pids.insert(
            "surface-1".to_string(),
            SurfacePid {
                pid: 1002,
                spawn_token: 2,
            },
        );

        assert!(!remove_surface_pid_for_spawn(&mut pids, "surface-1", 1));
        assert_eq!(pids["surface-1"].spawn_token, 2);

        assert!(remove_surface_pid_for_spawn(&mut pids, "surface-1", 2));
        assert!(pids.is_empty());
    }

    #[test]
    fn detects_visible_prompt_text() {
        assert!(looks_like_prompt("build finished\n> "));
        assert!(looks_like_prompt("? Continue (Y/n)"));
        assert!(looks_like_prompt("Do you want to proceed?"));
        assert!(!looks_like_prompt("ordinary terminal output"));
    }

    #[test]
    fn prompt_notification_ignores_closed_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, closed_surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            let split = model
                .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
                .unwrap();
            model.close_surface(&split.id).unwrap();
            (workspace.id, split.id)
        };

        let notification = create_prompt_notification_if_surface_exists(
            &model,
            &workspace_id,
            &closed_surface_id,
            "Continue?",
        );

        assert!(notification.is_none());
        assert!(model.lock().unwrap().list_notifications().is_empty());
    }

    #[test]
    fn prompt_notification_requires_surface_workspace_match() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let first = model.create_workspace("first", "/tmp/first");
            let second = model.create_workspace("second", "/tmp/second");
            (first.id, second.focused_surface_id)
        };

        let notification = create_prompt_notification_if_surface_exists(
            &model,
            &workspace_id,
            &surface_id,
            "Continue?",
        );

        assert!(notification.is_none());
        assert!(model.lock().unwrap().list_notifications().is_empty());
    }

    #[test]
    fn prompt_notification_records_live_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        let notification = create_prompt_notification_if_surface_exists(
            &model,
            &workspace_id,
            &surface_id,
            "Continue?",
        );

        assert!(notification.is_some());
        assert_eq!(model.lock().unwrap().list_notifications().len(), 1);
    }

    #[test]
    fn close_active_workspace_keeps_a_terminal_when_closing_last_workspace() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let closed_surface_id = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", &project_cwd);
            workspace.focused_surface_id
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        close_active_workspace(&state);

        let workspaces = model.lock().unwrap().list_workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "main");
        assert_eq!(workspaces[0].working_dir, project_cwd);
        assert!(terminal.sent_text(&closed_surface_id).is_err());
        let surfaces = terminal.surfaces().unwrap();
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].workspace_id, workspaces[0].id);
        assert_eq!(surfaces[0].cwd, project_cwd);
    }

    #[test]
    fn close_active_surface_keeps_old_surface_when_replacement_spawn_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(SecondSpawnFailsBackend::default());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", &project_cwd);
            (workspace.id, workspace.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        close_active_surface(&state);

        let model = model.lock().unwrap();
        let workspaces = model.list_workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, workspace_id);
        assert_eq!(workspaces[0].focused_surface_id, surface_id);
        let model_surfaces = model.list_surfaces(Some(&workspace_id));
        assert_eq!(model_surfaces.len(), 1);
        assert_eq!(model_surfaces[0].id, surface_id);
        let backend_surfaces = terminal.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, surface_id);
        assert!(model.list_notifications().iter().any(|notification| {
            notification.title == "Close Pane Failed" && notification.body.contains("spawn failed")
        }));
    }

    #[test]
    fn close_active_terminal_does_not_spawn_terminal_for_remaining_browser() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, terminal_id, browser_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", &project_cwd);
            let terminal_id = workspace.focused_surface_id.clone();
            let browser = model
                .open_browser(
                    &workspace.id,
                    "about:blank",
                    forktty_core::ProfileId::default(),
                    SplitAxis::Horizontal,
                )
                .unwrap();
            assert!(model.focus_surface(&terminal_id));
            (workspace.id, terminal_id, browser.id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        close_active_surface(&state);

        let model = model.lock().unwrap();
        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, browser_id);
        let model_surfaces = model.list_surfaces(Some(&workspace_id));
        assert_eq!(model_surfaces.len(), 1);
        assert_eq!(model_surfaces[0].id, browser_id);
        assert!(matches!(
            model_surfaces[0].kind,
            forktty_core::SurfaceKind::Browser { .. }
        ));
        assert!(terminal.surfaces().unwrap().is_empty());
        assert!(terminal.sent_text(&terminal_id).is_err());
    }

    #[test]
    fn focus_workspace_keeps_previous_workspace_when_spawn_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(SecondSpawnFailsBackend::default());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (first_workspace_id, second_workspace_id, second_surface_id) = {
            let mut model = model.lock().unwrap();
            let first = model.create_workspace("first", &project_cwd);
            let second = model.create_workspace("second", &project_cwd);
            (first.id, second.id, second.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        focus_workspace(&state, &first_workspace_id);

        let model = model.lock().unwrap();
        assert_eq!(
            model.active_workspace_id().as_deref(),
            Some(second_workspace_id.as_str())
        );
        assert!(model.list_notifications().iter().any(|notification| {
            notification.title == "Workspace Switch Failed"
                && notification.body.contains("spawn failed")
        }));
        let backend_surfaces = terminal.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, second_surface_id);
    }

    #[test]
    fn close_active_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(SecondSpawnFailsBackend::default());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let surface_id = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", &project_cwd);
            workspace.focused_surface_id
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        close_active_workspace(&state);

        let model = model.lock().unwrap();
        let workspaces = model.list_workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "project");
        assert_eq!(workspaces[0].working_dir, project_cwd);
        assert!(workspaces[0].active);
        let model_surfaces = model.list_surfaces(Some(&workspaces[0].id));
        assert_eq!(model_surfaces.len(), 1);
        assert_eq!(model_surfaces[0].id, surface_id);
        let backend_surfaces = terminal.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, surface_id);
        assert!(model.list_notifications().iter().any(|notification| {
            notification.title == "Close Workspace Failed"
                && notification.body.contains("spawn failed")
        }));
    }

    #[test]
    fn close_active_workspace_keeps_model_when_backend_close_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(CloseFailsBackend::default());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let surface_id = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", &project_cwd);
            workspace.focused_surface_id
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        close_active_workspace(&state);

        let model = model.lock().unwrap();
        let workspaces = model.list_workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "project");
        assert_eq!(model.list_surfaces(Some(&workspaces[0].id)).len(), 1);
        assert_eq!(terminal.surfaces().unwrap().len(), 1);
        assert!(terminal
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id));
        assert!(model
            .list_notifications()
            .iter()
            .any(
                |notification| notification.title == "Close Workspace Failed"
                    && notification.body.contains("close failed")
            ));
    }

    #[test]
    fn close_worktree_workspace_keeps_model_when_backend_close_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_path_buf();
        let fallback_dir = tempfile::tempdir().unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (tx, rx) = mpsc::channel();
        let terminal = Arc::new(GtkVteBackend::new(tx));
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_worktree_workspace(
                "feature/test",
                &project_cwd,
                "feature/test",
                "feature-test",
            );
            (workspace.id, workspace.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();
        drop(rx);

        let error =
            close_workspace_by_worktree_name(&state, "feature-test", fallback_dir.path().into())
                .unwrap_err()
                .to_string();

        assert!(error.contains("sending on a closed channel"));
        let model = model.lock().unwrap();
        assert!(model
            .list_workspaces()
            .iter()
            .any(|workspace| workspace.id == workspace_id));
        assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
        assert!(terminal
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id));
    }

    #[test]
    fn close_last_worktree_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("feature/gtk-remove-spawn-{}", std::process::id());
        let info =
            worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
        let worktree_cwd = PathBuf::from(&info.path);
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(SecondSpawnFailsBackend::default());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_worktree_workspace(
                &info.branch,
                &worktree_cwd,
                &info.branch,
                &info.worktree_name,
            );
            (workspace.id, workspace.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        worktree::remove(repo_dir.path().to_str().unwrap(), &branch_name, false).unwrap();
        let error =
            close_workspace_by_worktree_name(&state, &info.worktree_name, repo_dir.path().into())
                .unwrap_err()
                .to_string();

        assert!(error.contains("spawn failed"), "{error}");
        let model = model.lock().unwrap();
        let workspaces = model.list_workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, workspace_id);
        assert!(workspaces[0].active);
        assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
        let backend_surfaces = terminal.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, surface_id);
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn worktree_create_removes_created_worktree_when_spawn_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("feature/spawn-rollback-{}", std::process::id());
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        {
            let mut model = model.lock().unwrap();
            model.create_workspace("repo", repo_dir.path());
        }
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let terminal = Arc::new(GtkVteBackend::new(tx));
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error =
            open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err();

        assert!(error.contains("sending on a closed channel"));
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
        let repo = Repository::open(repo_dir.path()).unwrap();
        assert!(repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .is_err());
        let model = model.lock().unwrap();
        assert_eq!(model.list_workspaces().len(), 1);
        assert!(model
            .list_workspaces()
            .iter()
            .all(|workspace| workspace.git_branch != branch_name));
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
    fn active_layout_signature_changes_when_model_focus_changes() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (first_surface_id, second_surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            let first_surface_id = workspace.focused_surface_id.clone();
            let second = model
                .split_surface(&first_surface_id, SplitAxis::Horizontal)
                .unwrap();
            (first_surface_id, second.id)
        };
        let before = active_layout_snapshot(&model).unwrap().0;

        assert!(model.lock().unwrap().focus_surface(&first_surface_id));
        let after = active_layout_snapshot(&model).unwrap().0;

        assert_ne!(before, after);
        assert!(before.contains(&format!("focus({second_surface_id})")));
        assert!(after.contains(&format!("focus({first_surface_id})")));
    }

    #[test]
    fn restart_surface_does_not_spawn_terminal_for_browser_pane() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, browser_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", "/tmp/project");
            let browser = model
                .open_browser(
                    &workspace.id,
                    "https://example.com",
                    forktty_core::ProfileId::default(),
                    SplitAxis::Horizontal,
                )
                .unwrap();
            (workspace.id, browser.id)
        };

        assert!(!restart_surface(&state, &browser_id));

        assert!(terminal.surfaces().unwrap().is_empty());
        let model = model.lock().unwrap();
        assert!(matches!(
            model.surface(&browser_id).unwrap().kind,
            forktty_core::SurfaceKind::Browser { .. }
        ));
        assert!(model.list_status(&workspace_id).is_empty());
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

        assert!(is_executable_file(Path::new(&shell)));
    }

    #[test]
    fn socket_path_env_ignores_blank_and_relative_values() {
        assert_eq!(socket_path_from_env(None), default_socket_path());
        assert_eq!(
            socket_path_from_env(Some("  /tmp/forktty-custom.sock  ".to_string())),
            PathBuf::from("/tmp/forktty-custom.sock")
        );
        assert_eq!(
            socket_path_from_env(Some("  ".to_string())),
            default_socket_path()
        );
        assert_eq!(
            socket_path_from_env(Some("relative.sock".to_string())),
            default_socket_path()
        );
    }

    #[test]
    fn builds_terminal_font_description_from_config() {
        let mut config = config::AppConfig::default();
        config.appearance.font_family = "JetBrains Mono".to_string();
        config.appearance.font_size = 16;

        let description =
            terminal_font_description_with_family(&config, "JetBrains Mono".to_string());

        assert!(description.to_string().contains("JetBrains Mono"));
        assert!(description.to_string().contains("16"));
    }

    #[test]
    fn terminal_theme_system_follows_color_scheme() {
        let mut config = config::AppConfig::default();
        config.general.theme_source = "light".to_string();
        config.appearance.terminal_theme = config::TERMINAL_THEME_SYSTEM.to_string();

        assert_eq!(terminal_colors_for_config(&config).background, "#fbfbfe");

        config.general.theme_source = "dark".to_string();

        assert_eq!(terminal_colors_for_config(&config).background, "#1e1e2e");
    }

    #[test]
    fn named_terminal_theme_overrides_light_color_scheme() {
        let mut config = config::AppConfig::default();
        config.general.theme_source = "light".to_string();
        config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();

        assert_eq!(terminal_colors_for_config(&config).background, "#282a36");
    }

    #[test]
    fn terminal_theme_presets_use_expected_ansi_values() {
        let mut config = config::AppConfig::default();

        config.appearance.terminal_theme = config::TERMINAL_THEME_CATPPUCCIN_MOCHA.to_string();
        assert_eq!(terminal_colors_for_config(&config).ansi[5], "#f5c2e7");

        config.appearance.terminal_theme = config::TERMINAL_THEME_ROSE_PINE.to_string();
        assert_eq!(terminal_colors_for_config(&config).ansi[15], "#e0def4");

        config.appearance.terminal_theme = config::TERMINAL_THEME_TOKYO_NIGHT.to_string();
        assert_eq!(terminal_colors_for_config(&config).ansi[9], "#ff899d");

        config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();
        assert_eq!(terminal_colors_for_config(&config).ansi[7], "#f8f8f2");
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
    fn dedupes_font_family_names() {
        let families = dedupe_font_family_names([
            " JetBrainsMono Nerd Font Mono ".to_string(),
            "JetBrainsMono Nerd Font Mono".to_string(),
            "".to_string(),
            "Noto Sans Mono".to_string(),
        ]);

        assert_eq!(families.len(), 2);
        assert!(families.contains(&"JetBrainsMono Nerd Font Mono".to_string()));
        assert!(families.contains(&"Noto Sans Mono".to_string()));
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

    #[test]
    fn gtk_worktree_actions_require_active_workspace() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        for result in [
            active_workspace_cwd_string(&state),
            open_worktree_from_gtk(&state, "feature/test", WorktreeAction::Create)
                .map(|_| String::new()),
            merge_worktree_from_gtk(&state, "feature/test"),
            remove_worktree_from_gtk(&state, "feature/test").map(|_| String::new()),
        ] {
            assert!(result
                .unwrap_err()
                .contains("No active workspace is available"));
        }
    }
}
