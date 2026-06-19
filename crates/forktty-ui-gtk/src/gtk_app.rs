use adw::prelude::*;
use forktty_core::{
    close_desktop_notification, command_safety::is_executable_file, config, dispatch_notification,
    session, validate_worktree_name, worktree, LogLevel, NotificationItem, NotificationKind,
    PaneNode, ProgressEntry, SplitAxis, StatusEntry, Surface, TerminalNotificationMetadata,
    WorkspaceModel, WorkspaceSelector, WorktreeNameError,
};
#[cfg(test)]
use forktty_socket::default_socket_path;
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, serve, socket_path_from_env, SocketAppState,
};
use forktty_terminal::{
    SpawnRequest, TerminalBackend, TerminalError, TerminalSurfaceState, TerminalTextCapture,
    TerminalTextSnapshot, TerminalTextSnapshotParts,
};
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
#[cfg(feature = "browser")]
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.forktty.forktty";
const NOTIFICATION_DEDUPE_WINDOW: Duration = Duration::from_secs(12);
const TERMINAL_METADATA_NOTIFICATION_INTERVAL: Duration = Duration::from_secs(10);
const PANED_RATIO_APPLY_FRAMES: u8 = 8;
const PANED_RATIO_MAX_FRAMES: u8 = 30;
const SESSION_RESIZE_SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
const SPLIT_VERTICAL_SHORTCUT: &str = "Ctrl+Shift+E";
const SPLIT_VERTICAL_ACCEL: &str = "<Control><Shift>E";
const PREVIOUS_TAB_SHORTCUT: &str = "Ctrl+PageUp";
const PREVIOUS_TAB_ACCEL: &str = "<Control>Page_Up";
const PREVIOUS_TAB_KP_ACCEL: &str = "<Control>KP_Page_Up";
const NEXT_TAB_SHORTCUT: &str = "Ctrl+PageDown";
const NEXT_TAB_ACCEL: &str = "<Control>Page_Down";
const NEXT_TAB_KP_ACCEL: &str = "<Control>KP_Page_Down";
const FIRST_TAB_SHORTCUT: &str = "Ctrl+Home";
const FIRST_TAB_ACCEL: &str = "<Control>Home";
const FIRST_TAB_KP_ACCEL: &str = "<Control>KP_Home";
const LAST_TAB_SHORTCUT: &str = "Ctrl+End";
const LAST_TAB_ACCEL: &str = "<Control>End";
const LAST_TAB_KP_ACCEL: &str = "<Control>KP_End";
const RESTART_PANE_SHORTCUT: &str = "Ctrl+Shift+R";
const RESTART_PANE_ACCEL: &str = "<Control><Shift>R";
const TERMINAL_ZOOM_IN_SHORTCUT: &str = "Ctrl++ / Ctrl+=";
const TERMINAL_ZOOM_OUT_SHORTCUT: &str = "Ctrl+-";
const TERMINAL_ZOOM_RESET_SHORTCUT: &str = "Ctrl+0";
const EMPTY_LAYOUT_SIGNATURE: &str = "empty-layout";
const GH_PR_VIEW_TIMEOUT: Duration = Duration::from_secs(8);
const GH_PR_VIEW_MAX_STDOUT_BYTES: u64 = 64 * 1024;

mod actions;
mod agent_setup;
mod agents_panel;
mod app;
mod backend;
#[cfg(feature = "browser")]
mod browser_bridge;
mod command_palette;
mod controller;
pub(crate) mod ghostty_gtk_embed;
mod ghostty_gtk_probe;
mod layout;
mod notifications_panel;
mod pane_chrome;
mod placeholders;
mod settings_dialog;
mod sidebar;
mod socket_server;
#[allow(dead_code)]
mod terminal_appearance;
#[allow(dead_code)]
mod terminal_clipboard;
mod terminal_geometry;
mod terminal_input;
mod terminal_links;
#[allow(dead_code)]
mod terminal_renderer;
#[allow(dead_code)]
mod terminal_runtime;
mod terminal_search;
#[allow(dead_code)]
mod terminal_signals;
#[allow(dead_code)]
mod terminal_widget;
mod ui_common;
mod updater;
mod welcome;
mod workspace_dialogs;
mod workspace_menu;
mod workspace_ops;
mod workspace_popover;
mod worktree_dialog;

use actions::*;
use agent_setup::*;
use agents_panel::*;
use app::*;
use backend::*;
#[cfg(feature = "browser")]
use browser_bridge::*;
use command_palette::*;
use controller::*;
use ghostty_gtk_embed::*;
use layout::*;
use notifications_panel::*;
use pane_chrome::*;
use placeholders::*;
use settings_dialog::*;
use sidebar::*;
use socket_server::*;
use terminal_appearance::*;
use terminal_clipboard::*;
use terminal_input::*;
use terminal_renderer::*;
use terminal_runtime::*;
use terminal_search::*;
use terminal_signals::*;
use terminal_widget::*;
use ui_common::*;
use updater::*;
use welcome::*;
use workspace_dialogs::*;
use workspace_menu::*;
use workspace_ops::*;
use workspace_popover::*;
use worktree_dialog::*;

pub fn run() {
    install_gtk_runtime_defaults();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
}

pub fn run_ghostty_gtk_probe() -> i32 {
    ghostty_gtk_probe::run()
}

#[cfg(test)]
mod tests;
