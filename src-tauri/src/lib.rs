mod config;
mod notification;
mod output_scanner;
mod pty_manager;
mod session;
mod socket_api;
mod worktree;

use base64::Engine;
use output_scanner::{OutputScanner, ScanEvent};
use pty_manager::PtyManager;
use serde::Serialize;
use std::io::Read;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State};

struct AppState {
    pty_manager: Arc<Mutex<PtyManager>>,
    socket_pending: socket_api::PendingRequests,
    socket_path: String,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "data")]
enum PtyEvent {
    Output(String),
    Eof,
    Error(String),
    Scan(ScanEvent),
}

#[tauri::command]
fn pty_spawn(
    state: State<'_, AppState>,
    on_output: Channel<PtyEvent>,
    cwd: Option<String>,
    workspace_id: Option<String>,
    surface_id: Option<String>,
) -> Result<u32, String> {
    let shell = config::load_config()
        .inspect_err(|e| eprintln!("Warning: failed to load config, using default shell: {e}"))
        .ok()
        .map(|c| c.general.shell)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));

    let shell_path = std::path::Path::new(&shell);
    if !shell_path.is_absolute() || !shell_path.exists() {
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

    let (id, reader) = {
        let mut mgr = state
            .pty_manager
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        mgr.spawn(&shell, 80, 24, cwd.as_deref(), Some(&env_refs))
            .map_err(|e| e.to_string())?
    };

    tauri::async_runtime::spawn_blocking(move || {
        read_pty_output(reader, on_output);
    });

    Ok(id)
}

fn read_pty_output(mut reader: Box<dyn Read + Send>, channel: Channel<PtyEvent>) {
    let mut buf = [0u8; 4096];
    let engine = base64::engine::general_purpose::STANDARD;
    let mut scanner = OutputScanner::new();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                let _ = channel.send(PtyEvent::Eof);
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
                break;
            }
            Err(e) => {
                let _ = channel.send(PtyEvent::Error(e.to_string()));
                break;
            }
        }
    }
}

#[tauri::command]
fn pty_write(state: State<'_, AppState>, id: u32, data: String) -> Result<(), String> {
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
    mgr.kill(id).map_err(|e| e.to_string())
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

// --- Socket bridge: frontend responds to bridged requests ---

#[tauri::command]
fn socket_respond(state: State<'_, AppState>, id: String, result: serde_json::Value) {
    socket_api::resolve_request(&state.socket_pending, &id, result);
}

// --- Notification commands ---

#[tauri::command]
fn send_desktop_notification(title: String, body: String) -> Result<(), String> {
    notification::send_desktop(&title, &body)
}

#[tauri::command]
fn send_custom_notification(command: String, title: String, body: String) -> Result<(), String> {
    notification::run_custom_command(&command, &title, &body)
}

// --- Worktree commands ---

fn cwd_string() -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;

    // AppImage mounts at /tmp/.mount_*, which is not a useful CWD.
    // Fall back to $HOME when we detect this.
    if cwd.starts_with("/tmp/.mount_") {
        return std::env::var("HOME").map_err(|e| format!("No HOME: {e}"));
    }

    Ok(cwd)
}

#[tauri::command]
fn worktree_create(name: String, layout: Option<String>) -> Result<worktree::WorktreeInfo, String> {
    let layout = layout.as_deref().unwrap_or("nested");
    worktree::create(&cwd_string()?, &name, layout).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_list() -> Result<Vec<worktree::WorktreeInfo>, String> {
    worktree::list(&cwd_string()?).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_remove(name: String) -> Result<(), String> {
    let cwd = cwd_string()?;
    let worktrees = worktree::list(&cwd).map_err(|e| e.to_string())?;
    if let Some(wt) = worktrees.iter().find(|w| w.name == name) {
        if let Ok(verified) = verify_repo_path(&wt.path) {
            let _ = worktree::run_hook(&verified, "teardown");
        }
    }
    worktree::remove(&cwd, &name, true).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_merge(name: String) -> Result<String, String> {
    worktree::merge(&cwd_string()?, &name).map_err(|e| e.to_string())
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
    if !canonical.starts_with(workdir) {
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
fn git_list_branches() -> Result<Vec<worktree::BranchInfo>, String> {
    let cwd = cwd_string()?;
    worktree::list_branches(&cwd).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_attach(
    branch_name: String,
    layout: Option<String>,
) -> Result<worktree::WorktreeInfo, String> {
    let cwd = cwd_string()?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK DMA-BUF renderer causes "Error 71 (Protocol error)" on Wayland.
    // Disable it before GTK initializes. Users can override via env.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // WebKitGTK routes localhost through http_proxy, causing blank window in dev.
    // Ensure localhost is excluded from proxy.
    if std::env::var_os("http_proxy").is_some() || std::env::var_os("https_proxy").is_some() {
        let no_proxy = std::env::var("no_proxy").unwrap_or_default();
        if !no_proxy.contains("localhost") {
            let new_val = if no_proxy.is_empty() {
                "localhost,127.0.0.1".to_string()
            } else {
                format!("{no_proxy},localhost,127.0.0.1")
            };
            std::env::set_var("no_proxy", &new_val);
        }
    }

    let _ = session::prune_old_logs(30);
    let _ = session::write_log("INFO", "ForkTTY starting");

    let socket_path =
        std::env::var("FORKTTY_SOCKET_PATH").unwrap_or_else(|_| socket_api::default_socket_path());

    let pty_manager = Arc::new(Mutex::new(PtyManager::new()));
    let socket_pending = socket_api::PendingRequests::default();

    let pty_mgr_for_socket = pty_manager.clone();
    let pending_for_socket = socket_pending.clone();
    let socket_path_clone = socket_path.clone();

    tauri::Builder::default()
        .manage(AppState {
            pty_manager,
            socket_pending,
            socket_path,
        })
        .setup(move |app| {
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

            let handle = app.handle().clone();
            // Start socket server in background thread with its own tokio runtime
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.block_on(socket_api::run(
                    socket_path_clone,
                    handle,
                    pty_mgr_for_socket,
                    pending_for_socket,
                ));
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            get_git_branch,
            get_cwd,
            get_socket_path,
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
            save_session,
            load_session,
            write_log,
            update_tray_tooltip
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // Cleanup: remove socket file on exit
    let socket_cleanup =
        std::env::var("FORKTTY_SOCKET_PATH").unwrap_or_else(|_| socket_api::default_socket_path());
    let _ = std::fs::remove_file(&socket_cleanup);
    let _ = session::write_log("INFO", "ForkTTY shutdown complete");
}
