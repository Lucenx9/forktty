use adw::prelude::*;
use forktty_core::{
    command_safety::is_executable_file, config, dispatch_notification, session,
    validate_worktree_name, worktree, LogLevel, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, Surface, WorkspaceModel, WorkspaceSelector,
    WorktreeNameError,
};
#[cfg(test)]
use forktty_socket::default_socket_path;
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, serve, socket_path_from_env, SocketAppState,
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
#[cfg(feature = "browser")]
use serde_json::{json, Value};
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

const APP_ID: &str = "dev.forktty.forktty";
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
const SESSION_RESIZE_SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
const SPLIT_VERTICAL_SHORTCUT: &str = "Ctrl+Shift+E";
const SPLIT_VERTICAL_ACCEL: &str = "<Control><Shift>E";
const PREVIOUS_TAB_SHORTCUT: &str = "Ctrl+PageUp";
const PREVIOUS_TAB_ACCEL: &str = "<Control>Page_Up";
const NEXT_TAB_SHORTCUT: &str = "Ctrl+PageDown";
const NEXT_TAB_ACCEL: &str = "<Control>Page_Down";
const FIRST_TAB_SHORTCUT: &str = "Ctrl+Home";
const FIRST_TAB_ACCEL: &str = "<Control>Home";
const LAST_TAB_SHORTCUT: &str = "Ctrl+End";
const LAST_TAB_ACCEL: &str = "<Control>End";
const RESTART_PANE_SHORTCUT: &str = "Ctrl+Shift+R";
const RESTART_PANE_ACCEL: &str = "<Control><Shift>R";
const EMPTY_LAYOUT_SIGNATURE: &str = "empty-layout";
const GH_PR_VIEW_TIMEOUT: Duration = Duration::from_secs(8);
const GH_PR_VIEW_MAX_STDOUT_BYTES: u64 = 64 * 1024;

mod actions;
mod app;
mod backend;
#[cfg(feature = "browser")]
mod browser_bridge;
mod command_palette;
mod controller;
mod layout;
mod notifications_panel;
mod pane_chrome;
mod placeholders;
mod settings_dialog;
mod sidebar;
mod socket_server;
mod terminal_appearance;
mod terminal_signals;
mod ui_common;
mod workspace_dialogs;
mod workspace_menu;
mod workspace_ops;
mod workspace_popover;
mod worktree_dialog;

use actions::*;
use app::*;
use backend::*;
#[cfg(feature = "browser")]
use browser_bridge::*;
use command_palette::*;
use controller::*;
use layout::*;
use notifications_panel::*;
use pane_chrome::*;
use placeholders::*;
use settings_dialog::*;
use sidebar::*;
use socket_server::*;
use terminal_appearance::*;
use terminal_signals::*;
use ui_common::*;
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

#[cfg(test)]
mod tests;
