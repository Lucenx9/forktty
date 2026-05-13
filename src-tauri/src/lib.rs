mod config;
mod notification;
mod output_scanner;
mod pty_manager;
mod session;
mod socket_api;
mod worktree;

use base64::Engine;
use dpi::{PhysicalPosition, PhysicalSize};
use output_scanner::{OutputScanner, ScanEvent};
use pty_manager::{PtyError, PtyManager};
use serde::Serialize;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State};
#[cfg(desktop)]
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

struct AppState {
    pty_manager: Arc<Mutex<PtyManager>>,
    socket_pending: socket_api::PendingRequests,
    socket_frontend: Arc<socket_api::FrontendState>,
    socket_path: String,
    quake_state: Arc<Mutex<QuakeWindowState>>,
}

const QUAKE_SHORTCUT_LABEL: &str = "F12";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowMode {
    Normal,
    Quake,
}

impl WindowMode {
    fn from_config_value(value: &str) -> Self {
        match value {
            "quake" => Self::Quake,
            _ => Self::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct WindowBounds {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    maximized: bool,
}

#[derive(Debug)]
struct QuakeWindowState {
    mode: WindowMode,
    restore_bounds: Option<WindowBounds>,
    shortcut_registered: bool,
}

impl Default for QuakeWindowState {
    fn default() -> Self {
        Self {
            mode: WindowMode::Normal,
            restore_bounds: None,
            shortcut_registered: false,
        }
    }
}

#[cfg(desktop)]
fn quake_shortcut() -> Shortcut {
    Shortcut::new(None, Code::F12)
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "data")]
enum PtyEvent {
    Output(String),
    Eof,
    Error(String),
    Scan(ScanEvent),
}

fn is_executable_file(path: &std::path::Path) -> bool {
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

#[cfg(desktop)]
fn capture_window_bounds(window: &tauri::WebviewWindow) -> Result<WindowBounds, String> {
    Ok(WindowBounds {
        position: window.outer_position().map_err(|e| e.to_string())?,
        size: window.outer_size().map_err(|e| e.to_string())?,
        maximized: window.is_maximized().map_err(|e| e.to_string())?,
    })
}

#[cfg(desktop)]
fn resolve_quake_bounds(window: &tauri::WebviewWindow) -> Result<WindowBounds, String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "No monitor available for quake mode".to_string())?;

    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let horizontal_margin = ((monitor_size.width as f64) * 0.02).round() as i32;
    let width = monitor_size
        .width
        .saturating_sub((horizontal_margin.max(0) as u32).saturating_mul(2))
        .max(960);
    let height = ((monitor_size.height as f64) * 0.62).round() as u32;

    Ok(WindowBounds {
        position: PhysicalPosition::new(monitor_position.x + horizontal_margin, monitor_position.y),
        size: PhysicalSize::new(width, height.max(420)),
        maximized: false,
    })
}

#[cfg(desktop)]
fn apply_window_bounds(
    window: &tauri::WebviewWindow,
    bounds: WindowBounds,
    always_on_top: bool,
    skip_taskbar: bool,
) -> Result<(), String> {
    if window.is_maximized().map_err(|e| e.to_string())? {
        window.unmaximize().map_err(|e| e.to_string())?;
    }

    window
        .set_always_on_top(always_on_top)
        .map_err(|e| e.to_string())?;
    window
        .set_skip_taskbar(skip_taskbar)
        .map_err(|e| e.to_string())?;
    window.set_size(bounds.size).map_err(|e| e.to_string())?;
    window
        .set_position(bounds.position)
        .map_err(|e| e.to_string())?;

    if bounds.maximized {
        window.maximize().map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(desktop)]
fn ensure_quake_shortcut_registration(
    app: &tauri::AppHandle,
    quake_state: &Arc<Mutex<QuakeWindowState>>,
    should_register: bool,
) {
    let Ok(mut state) = quake_state.lock() else {
        return;
    };

    if should_register == state.shortcut_registered {
        return;
    }

    let manager = app.global_shortcut();
    let shortcut = quake_shortcut();
    let result = if should_register {
        manager.register(shortcut)
    } else {
        manager.unregister(shortcut)
    };

    match result {
        Ok(()) => state.shortcut_registered = should_register,
        Err(err) => {
            let _ = session::write_log(
                "WARN",
                &format!("Quake shortcut {QUAKE_SHORTCUT_LABEL} unavailable: {err}"),
            );
        }
    }
}

#[cfg(desktop)]
fn sync_window_mode_internal(
    app: &tauri::AppHandle,
    quake_state: &Arc<Mutex<QuakeWindowState>>,
    next_mode: WindowMode,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    {
        let mut state = quake_state.lock().map_err(|e| e.to_string())?;
        if state.mode == next_mode {
            if next_mode == WindowMode::Quake {
                let bounds = resolve_quake_bounds(&window)?;
                apply_window_bounds(&window, bounds, true, true)?;
            }
        } else {
            match next_mode {
                WindowMode::Quake => {
                    if state.restore_bounds.is_none() {
                        state.restore_bounds = Some(capture_window_bounds(&window)?);
                    }
                    let quake_bounds = resolve_quake_bounds(&window)?;
                    apply_window_bounds(&window, quake_bounds, true, true)?;
                }
                WindowMode::Normal => {
                    window.set_always_on_top(false).map_err(|e| e.to_string())?;
                    window.set_skip_taskbar(false).map_err(|e| e.to_string())?;
                    if let Some(bounds) = state.restore_bounds.take() {
                        apply_window_bounds(&window, bounds, false, false)?;
                    }
                }
            }

            state.mode = next_mode;
        }
    }

    ensure_quake_shortcut_registration(app, quake_state, next_mode == WindowMode::Quake);
    Ok(())
}

#[cfg(not(desktop))]
fn sync_window_mode_internal(
    _app: &tauri::AppHandle,
    _quake_state: &Arc<Mutex<QuakeWindowState>>,
    _next_mode: WindowMode,
) -> Result<(), String> {
    Ok(())
}

#[cfg(desktop)]
fn toggle_quake_window_internal(
    app: &tauri::AppHandle,
    quake_state: &Arc<Mutex<QuakeWindowState>>,
) -> Result<(), String> {
    let state = quake_state.lock().map_err(|e| e.to_string())?;
    if state.mode != WindowMode::Quake {
        return Ok(());
    }
    drop(state);

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    if window.is_visible().map_err(|e| e.to_string())? {
        window.hide().map_err(|e| e.to_string())
    } else {
        let quake_bounds = resolve_quake_bounds(&window)?;
        apply_window_bounds(&window, quake_bounds, true, true)?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())
    }
}

#[cfg(not(desktop))]
fn toggle_quake_window_internal(
    _app: &tauri::AppHandle,
    _quake_state: &Arc<Mutex<QuakeWindowState>>,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn pty_spawn(
    state: State<'_, AppState>,
    on_output: Channel<PtyEvent>,
    cwd: Option<String>,
    workspace_id: Option<String>,
    surface_id: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<u32, String> {
    let shell = config::load_config()
        .inspect_err(|e| eprintln!("Warning: failed to load config, using default shell: {e}"))
        .ok()
        .map(|c| c.general.shell)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));

    let shell_path = std::path::Path::new(&shell);
    if !is_executable_file(shell_path) {
        return Err(format!("Invalid shell path: {shell}"));
    }

    let socket_path = state.socket_path.clone();

    // Build env vars for the spawned shell
    let mut env_pairs: Vec<(String, String)> = Vec::new();
    if let Some(ref ws_id) = workspace_id {
        env_pairs.push(("FORKTTY_WORKSPACE_ID".to_string(), ws_id.clone()));
    }
    if let Some(ref sf_id) = surface_id {
        env_pairs.push(("FORKTTY_SURFACE_ID".to_string(), sf_id.clone()));
    }
    env_pairs.push(("FORKTTY_SOCKET_PATH".to_string(), socket_path));

    let env_refs: Vec<(&str, &str)> = env_pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let pty_manager_for_reader = state.pty_manager.clone();

    let (id, reader) = {
        let mut mgr = state
            .pty_manager
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        mgr.spawn(
            &shell,
            cols.unwrap_or(120),
            rows.unwrap_or(30),
            cwd.as_deref(),
            Some(&env_refs),
        )
        .map_err(|e| e.to_string())?
    };

    tauri::async_runtime::spawn_blocking(move || {
        read_pty_output(id, reader, on_output, pty_manager_for_reader);
    });

    Ok(id)
}

fn read_pty_output(
    id: u32,
    mut reader: Box<dyn Read + Send>,
    channel: Channel<PtyEvent>,
    pty_manager: Arc<Mutex<PtyManager>>,
) {
    let mut buf = [0u8; 4096];
    let engine = base64::engine::general_purpose::STANDARD;
    let mut scanner = OutputScanner::new();
    let mut should_reap = false;

    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                let _ = channel.send(PtyEvent::Eof);
                should_reap = true;
                break;
            }
            Ok(n) => {
                let data = &buf[..n];
                let scan_events = scanner.scan(data);

                let encoded = engine.encode(data);
                if channel.send(PtyEvent::Output(encoded)).is_err() {
                    break;
                }

                for event in scan_events {
                    if channel.send(PtyEvent::Scan(event)).is_err() {
                        break;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                continue; // Retry on EINTR
            }
            Err(e) if e.raw_os_error() == Some(libc::EIO) => {
                // EIO is normal on Linux when child exits — treat as EOF
                let _ = channel.send(PtyEvent::Eof);
                should_reap = true;
                break;
            }
            Err(e) => {
                let _ = channel.send(PtyEvent::Error(e.to_string()));
                break;
            }
        }
    }

    if should_reap {
        if let Ok(mut mgr) = pty_manager.lock() {
            mgr.reap(id);
        }
    }
}

#[tauri::command]
fn pty_write(state: State<'_, AppState>, id: u32, data: String) -> Result<(), String> {
    const PTY_WRITE_MAX_BYTES: usize = 262_144;
    if data.len() > PTY_WRITE_MAX_BYTES {
        return Err(format!(
            "PTY write exceeds {} KiB",
            PTY_WRITE_MAX_BYTES / 1024
        ));
    }
    let mgr = state
        .pty_manager
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;
    mgr.write(id, data.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn pty_resize(state: State<'_, AppState>, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let mgr = state
        .pty_manager
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;
    mgr.resize(id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
fn pty_kill(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    let mut mgr = state
        .pty_manager
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;
    match mgr.kill(id) {
        Ok(()) | Err(PtyError::NotFound(_)) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn pty_get_cwd(state: State<'_, AppState>, id: u32) -> Result<String, String> {
    let mgr = state
        .pty_manager
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;
    mgr.cwd(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_git_branch(cwd: String) -> Result<String, String> {
    let repo = match git2::Repository::discover(&cwd) {
        Ok(r) => r,
        Err(_) => return Ok(String::new()),
    };
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok(String::new()),
    };
    Ok(head.shorthand().unwrap_or("detached").to_string())
}

#[tauri::command]
fn get_cwd() -> Result<String, String> {
    cwd_string()
}

#[tauri::command]
fn get_socket_path(state: State<'_, AppState>) -> String {
    state.socket_path.clone()
}

#[tauri::command]
fn socket_frontend_ready(state: State<'_, AppState>) {
    state.socket_frontend.mark_ready();
}

// --- Socket bridge: frontend responds to bridged requests ---

#[tauri::command]
fn socket_respond(state: State<'_, AppState>, id: String, result: serde_json::Value) {
    socket_api::resolve_request(&state.socket_pending, &id, result);
}

// --- Notification commands ---

#[tauri::command]
fn send_desktop_notification(
    title: String,
    body: String,
    play_sound: Option<bool>,
) -> Result<(), String> {
    notification::send_desktop(&title, &body, play_sound.unwrap_or(true))
}

#[tauri::command]
fn send_custom_notification(command: String, title: String, body: String) -> Result<(), String> {
    notification::run_custom_command(&command, &title, &body)
}

// --- Worktree commands ---

/// Return a sensible initial working directory.
/// Prefers $HOME over the process CWD because desktop launchers, file managers,
/// and AppImage mounts often set CWD to a non-project directory.
pub(crate) fn cwd_string() -> Result<String, String> {
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::Path::new(&home);
        if path.is_absolute() && path.exists() {
            return Ok(home);
        }
    }

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;

    // AppImage mounts at /tmp/.mount_*, which is not a useful CWD.
    if cwd.starts_with("/tmp/.mount_") {
        return Err("No usable CWD and $HOME is not set".to_string());
    }

    Ok(cwd)
}

fn resolved_repo_cwd(cwd: Option<String>) -> Result<String, String> {
    let path = match cwd {
        Some(p) if !p.trim().is_empty() => p,
        _ => cwd_string()?,
    };
    // Validate the path is inside a git repository before proceeding.
    // Without this, fallback CWDs like $HOME silently fail deep in worktree ops.
    if git2::Repository::discover(&path).is_err() {
        return Err(format!(
            "Not a git repository: {path} — open a terminal in a git project first"
        ));
    }
    Ok(path)
}

pub(crate) fn repo_commondir_for_path(path: &str) -> Result<std::path::PathBuf, String> {
    let repo = git2::Repository::discover(path).map_err(|_| {
        format!("Not a git repository: {path} — open a terminal in a git project first")
    })?;
    if repo.workdir().is_none() {
        return Err("Bare repository".to_string());
    }
    std::fs::canonicalize(repo.commondir())
        .map_err(|e| format!("Cannot resolve repository common dir: {e}"))
}

pub(crate) fn repo_root_for_path(path: &str) -> Result<String, String> {
    let common_dir = repo_commondir_for_path(path)?;
    let repo_root = common_dir
        .parent()
        .ok_or_else(|| "Repository common dir has no parent".to_string())?;
    let canonical_root =
        std::fs::canonicalize(repo_root).map_err(|e| format!("Cannot resolve repo root: {e}"))?;
    canonical_root
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "Repository root is not valid UTF-8".to_string())
}

#[tauri::command]
fn worktree_create(
    name: String,
    layout: Option<String>,
    cwd: Option<String>,
) -> Result<worktree::WorktreeInfo, String> {
    let layout = layout.as_deref().unwrap_or("nested");
    worktree::create(&resolved_repo_cwd(cwd)?, &name, layout).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_list(cwd: Option<String>) -> Result<Vec<worktree::WorktreeInfo>, String> {
    worktree::list(&resolved_repo_cwd(cwd)?).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_remove(name: String, cwd: Option<String>) -> Result<String, String> {
    let cwd = resolved_repo_cwd(cwd)?;
    let fallback_working_dir = repo_root_for_path(&cwd)?;
    let plan = worktree::prepare_remove(&cwd, &name, true).map_err(|e| e.to_string())?;
    if let Ok(verified) = verify_repo_path(plan.worktree_path()) {
        let _ = worktree::run_hook(&verified, "teardown");
    }
    worktree::execute_remove(&cwd, &plan).map_err(|e| e.to_string())?;
    Ok(fallback_working_dir)
}

#[tauri::command]
fn worktree_merge(name: String, cwd: Option<String>) -> Result<String, String> {
    worktree::merge(&resolved_repo_cwd(cwd)?, &name).map_err(|e| e.to_string())
}

/// Canonicalize a path and verify it is inside a git repository's working directory.
/// Returns the canonical path string. This is a security boundary — prevents
/// arbitrary filesystem access or hook execution outside the repo.
pub(crate) fn verify_repo_path(path: &str) -> Result<String, String> {
    let canonical = std::fs::canonicalize(path).map_err(|e| format!("Invalid path: {e}"))?;
    let canonical_str = canonical.to_str().ok_or("Non-UTF-8 path")?;
    let repo = git2::Repository::discover(canonical_str)
        .map_err(|_| "Path is not inside a git repository".to_string())?;
    let workdir = repo.workdir().ok_or("Bare repository")?;
    let canonical_workdir =
        std::fs::canonicalize(workdir).map_err(|e| format!("Cannot resolve workdir: {e}"))?;
    if !canonical.starts_with(&canonical_workdir) {
        return Err("Path is outside the repository working directory".to_string());
    }
    Ok(canonical_str.to_string())
}

#[tauri::command]
fn worktree_status(path: String) -> Result<String, String> {
    let verified = verify_repo_path(&path)?;
    worktree::status(&verified).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_run_hook(worktree_path: String, hook_name: String) -> Result<Option<i32>, String> {
    let verified = verify_repo_path(&worktree_path)?;
    worktree::run_hook(&verified, &hook_name).map_err(|e| e.to_string())
}

#[tauri::command]
fn git_list_branches(cwd: Option<String>) -> Result<Vec<worktree::BranchInfo>, String> {
    let cwd = resolved_repo_cwd(cwd)?;
    worktree::list_branches(&cwd).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_attach(
    branch_name: String,
    layout: Option<String>,
    cwd: Option<String>,
) -> Result<worktree::WorktreeInfo, String> {
    let cwd = resolved_repo_cwd(cwd)?;
    let layout_str = layout.as_deref().unwrap_or("nested");
    worktree::attach(&cwd, &branch_name, layout_str).map_err(|e| e.to_string())
}

// --- Config commands ---

#[tauri::command]
fn get_config() -> Result<config::AppConfig, String> {
    config::load_config().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(config_data: config::AppConfig) -> Result<(), String> {
    config::save_config(&config_data).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_theme() -> Result<config::TerminalTheme, String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    Ok(config::resolve_theme(&cfg))
}

#[tauri::command]
fn sync_window_mode(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    window_mode: String,
) -> Result<(), String> {
    let mode = WindowMode::from_config_value(&window_mode);
    sync_window_mode_internal(&app, &state.quake_state, mode)
}

#[tauri::command]
fn toggle_quake_window(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    toggle_quake_window_internal(&app, &state.quake_state)
}

// --- Session commands ---

#[tauri::command]
fn save_session(data: session::SessionData) -> Result<(), String> {
    session::save_session(&data).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_session() -> Result<Option<session::SessionData>, String> {
    session::load_session().map_err(|e| e.to_string())
}

// --- Logging command ---

#[tauri::command]
fn write_log(level: String, message: String) -> Result<(), String> {
    session::write_log(&level, &message).map_err(|e| e.to_string())
}

#[tauri::command]
fn update_tray_tooltip(app: tauri::AppHandle, count: u32) -> Result<(), String> {
    use tauri::tray::TrayIconId;
    if let Some(tray) = app.tray_by_id(&TrayIconId::new("main-tray")) {
        let tooltip = if count > 0 {
            format!("ForkTTY ({count} unread)")
        } else {
            "ForkTTY".to_string()
        };
        tray.set_tooltip(Some(&tooltip))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

const LOCALHOST_NO_PROXY_ENTRIES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

fn append_no_proxy_hosts(existing: &str) -> String {
    let mut values: Vec<String> = existing
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    for host in LOCALHOST_NO_PROXY_ENTRIES {
        if !values.iter().any(|entry| entry.eq_ignore_ascii_case(host)) {
            values.push(host.to_string());
        }
    }

    values.join(",")
}

fn ensure_localhost_no_proxy_env() {
    let proxy_vars = ["http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY"];
    if !proxy_vars.iter().any(|key| std::env::var_os(key).is_some()) {
        return;
    }

    let current = std::env::var("no_proxy")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("NO_PROXY")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_default();

    let updated = append_no_proxy_hosts(&current);
    std::env::set_var("no_proxy", &updated);
    std::env::set_var("NO_PROXY", &updated);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK DMA-BUF renderer causes "Error 71 (Protocol error)" on Wayland.
    // Disable it before GTK initializes. Users can override via env.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    // WebKitGTK routes localhost through http_proxy, causing blank window in dev.
    // Ensure localhost is excluded from proxy.
    ensure_localhost_no_proxy_env();

    let _ = session::prune_old_logs(30);
    let _ = session::write_log("INFO", "ForkTTY starting");

    let (socket_path, socket_uses_default_parent_policy) =
        match std::env::var("FORKTTY_SOCKET_PATH") {
            Ok(path) => (path, false),
            Err(_) => (socket_api::default_socket_path(), true),
        };
    let socket_listener =
        match socket_api::bind_socket_listener(&socket_path, socket_uses_default_parent_policy) {
            Ok(listener) => listener,
            Err(err) => {
                let message = format!(
                    "Failed to initialize the local socket at {}: {err}",
                    socket_path
                );
                eprintln!("{message}");
                let _ = session::write_log("ERROR", &message);
                std::process::exit(1);
            }
        };

    let pty_manager = Arc::new(Mutex::new(PtyManager::new()));
    let socket_pending = socket_api::PendingRequests::default();
    let socket_frontend = Arc::new(socket_api::FrontendState::default());
    let quake_state = Arc::new(Mutex::new(QuakeWindowState::default()));

    let socket_path_for_cleanup = socket_path.clone();
    let pty_mgr_for_socket = pty_manager.clone();
    let pending_for_socket = socket_pending.clone();
    let frontend_for_socket = socket_frontend.clone();
    let quake_state_for_setup = quake_state.clone();

    tauri::Builder::default()
        .manage(AppState {
            pty_manager,
            socket_pending,
            socket_frontend,
            socket_path,
            quake_state,
        })
        .setup(move |app| {
            #[cfg(desktop)]
            {
                let quake_state_for_shortcut = quake_state_for_setup.clone();
                let shortcut = quake_shortcut();
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app, pressed_shortcut, event| {
                            if pressed_shortcut == &shortcut
                                && event.state() == ShortcutState::Pressed
                            {
                                let _ =
                                    toggle_quake_window_internal(app, &quake_state_for_shortcut);
                            }
                        })
                        .build(),
                )?;
            }

            // Build system tray icon (best-effort: may fail on Wayland without appindicator)
            match TrayIconBuilder::with_id("main-tray")
                .tooltip("ForkTTY")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .unwrap_or_else(|| tauri::image::Image::new(&[], 0, 0)),
                )
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { .. } = event {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)
            {
                Ok(_tray) => {}
                Err(e) => eprintln!("Tray icon unavailable (Wayland?): {e}"),
            }

            if let Ok(cfg) = config::load_config() {
                let initial_window_mode =
                    WindowMode::from_config_value(&cfg.appearance.window_mode);
                let _ = sync_window_mode_internal(
                    &app.handle().clone(),
                    &quake_state_for_setup,
                    initial_window_mode,
                );
            }

            let handle = app.handle().clone();
            // Start socket server in background thread with its own tokio runtime
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.block_on(socket_api::serve(
                    socket_listener,
                    handle,
                    pty_mgr_for_socket,
                    pending_for_socket,
                    frontend_for_socket,
                ));
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            pty_get_cwd,
            get_git_branch,
            get_cwd,
            get_socket_path,
            socket_frontend_ready,
            socket_respond,
            send_desktop_notification,
            send_custom_notification,
            worktree_create,
            worktree_list,
            worktree_remove,
            worktree_merge,
            worktree_status,
            worktree_run_hook,
            git_list_branches,
            worktree_attach,
            get_config,
            save_config,
            get_theme,
            sync_window_mode,
            toggle_quake_window,
            save_session,
            load_session,
            write_log,
            update_tray_tooltip
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // Cleanup: remove socket file on exit
    let _ = std::fs::remove_file(&socket_path_for_cleanup);
    let _ = session::write_log("INFO", "ForkTTY shutdown complete");
}

#[cfg(test)]
mod tests {
    use super::{
        append_no_proxy_hosts, repo_commondir_for_path, repo_root_for_path, verify_repo_path,
        LOCALHOST_NO_PROXY_ENTRIES,
    };
    use git2::Repository;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_repo(name: &str) -> (PathBuf, Repository) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo_path = std::env::temp_dir().join(format!(
            "forktty-lib-test-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&repo_path).unwrap();

        let repo = Repository::init(&repo_path).unwrap();
        fs::write(repo_path.join("note.txt"), "base\n").unwrap();
        commit_all(&repo, "initial");

        (repo_path, repo)
    }

    fn commit_all(repo: &Repository, message: &str) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();

        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok());

        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap();
    }

    #[test]
    fn append_no_proxy_hosts_adds_local_entries_once() {
        let updated = append_no_proxy_hosts("corp.local,localhost");

        assert!(updated.contains("corp.local"));
        for host in LOCALHOST_NO_PROXY_ENTRIES {
            assert_eq!(
                updated.matches(host).count(),
                1,
                "missing or duplicated {host}"
            );
        }
    }

    #[test]
    fn append_no_proxy_hosts_trims_empty_entries() {
        let updated = append_no_proxy_hosts(" ,127.0.0.1,, ");
        assert_eq!(updated, "127.0.0.1,localhost,::1");
    }

    #[test]
    fn repo_helpers_resolve_main_repo_identity_for_linked_worktrees() {
        let (repo_path, repo) = make_temp_repo("repo-root");
        let worktree_path = repo_path.with_file_name(format!(
            "{}-worktree",
            repo_path.file_name().unwrap().to_string_lossy()
        ));
        let _ = fs::remove_dir_all(&worktree_path);

        repo.worktree("feature", &worktree_path, None).unwrap();

        let repo_common_dir = fs::canonicalize(repo.path()).unwrap();
        assert_eq!(
            repo_commondir_for_path(worktree_path.to_str().unwrap()).unwrap(),
            repo_common_dir
        );
        assert_eq!(
            Path::new(&repo_root_for_path(worktree_path.to_str().unwrap()).unwrap()),
            fs::canonicalize(&repo_path).unwrap()
        );

        let _ = fs::remove_dir_all(&worktree_path);
        let _ = fs::remove_dir_all(&repo_path);
    }

    #[cfg(unix)]
    #[test]
    fn verify_repo_path_accepts_paths_reached_through_symlink() {
        let (repo_path, _repo) = make_temp_repo("verify-symlink");
        let link_path = repo_path.with_file_name(format!(
            "{}-link",
            repo_path.file_name().unwrap().to_string_lossy()
        ));
        let _ = fs::remove_file(&link_path);
        std::os::unix::fs::symlink(&repo_path, &link_path).unwrap();

        let verified = verify_repo_path(link_path.join("note.txt").to_str().unwrap()).unwrap();

        assert_eq!(
            PathBuf::from(verified),
            fs::canonicalize(repo_path.join("note.txt")).unwrap()
        );

        let _ = fs::remove_file(&link_path);
        let _ = fs::remove_dir_all(&repo_path);
    }
}
