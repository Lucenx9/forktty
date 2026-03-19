mod notification;
mod output_scanner;
mod pty_manager;
mod worktree;

use base64::Engine;
use output_scanner::{OutputScanner, ScanEvent};
use pty_manager::PtyManager;
use serde::Serialize;
use std::io::Read;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::State;

struct AppState {
    pty_manager: Mutex<PtyManager>,
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
) -> Result<u32, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    let (id, reader) = {
        let mut mgr = state
            .pty_manager
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        mgr.spawn(&shell, 80, 24, cwd.as_deref())
            .map_err(|e| e.to_string())?
    };

    // Background read loop: read PTY output, scan, send via channel
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

                // Run output scanner before forwarding to frontend
                let scan_events = scanner.scan(data);

                // Send the raw output
                let encoded = engine.encode(data);
                if channel.send(PtyEvent::Output(encoded)).is_err() {
                    break;
                }

                // Send any scan events
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
    worktree::status(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn worktree_run_hook(worktree_path: String, hook_name: String) -> Result<Option<i32>, String> {
    worktree::run_hook(&worktree_path, &hook_name).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            pty_manager: Mutex::new(PtyManager::new()),
        })
        .invoke_handler(tauri::generate_handler![
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            get_git_branch,
            get_cwd,
            send_desktop_notification,
            send_custom_notification,
            worktree_create,
            worktree_list,
            worktree_remove,
            worktree_merge,
            worktree_status,
            worktree_run_hook
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
