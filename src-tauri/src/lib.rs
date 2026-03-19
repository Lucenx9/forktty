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
use tauri::State;

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

    std::thread::spawn(move || {
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
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
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

#[tauri::command]
fn worktree_create(name: String, layout: Option<String>) -> Result<worktree::WorktreeInfo, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();
    let layout = layout.as_deref().unwrap_or("nested");
    worktree::create(&cwd, &name, layout).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_list() -> Result<Vec<worktree::WorktreeInfo>, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();
    worktree::list(&cwd).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_remove(name: String) -> Result<(), String> {
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();
    let worktrees = worktree::list(&cwd).map_err(|e| e.to_string())?;
    if let Some(wt) = worktrees.iter().find(|w| w.name == name) {
        let _ = worktree::run_hook(&wt.path, "teardown");
    }
    worktree::remove(&cwd, &name, true).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_merge(name: String) -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();
    worktree::merge(&cwd, &name).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_status(path: String) -> Result<String, String> {
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("Invalid path: {e}"))?;
    if let Some(home) = dirs::home_dir() {
        if !canonical.starts_with(&home) {
            return Err("Path must be inside home directory".to_string());
        }
    }
    worktree::status(canonical.to_str().unwrap_or("")).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_run_hook(worktree_path: String, hook_name: String) -> Result<Option<i32>, String> {
    let canonical =
        std::fs::canonicalize(&worktree_path).map_err(|e| format!("Invalid path: {e}"))?;
    if let Some(home) = dirs::home_dir() {
        if !canonical.starts_with(&home) {
            return Err("Path must be inside home directory".to_string());
        }
    }
    worktree::run_hook(canonical.to_str().unwrap_or(""), &hook_name).map_err(|e| e.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            get_config,
            save_config,
            get_theme,
            save_session,
            load_session,
            write_log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
