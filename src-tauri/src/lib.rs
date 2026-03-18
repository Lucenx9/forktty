mod pty_manager;

use base64::Engine;
use pty_manager::PtyManager;
use std::io::Read;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::State;

struct AppState {
    pty_manager: Mutex<PtyManager>,
}

#[tauri::command]
fn pty_spawn(state: State<'_, AppState>, on_output: Channel<String>) -> Result<u32, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    let (id, reader) = {
        let mut mgr = state
            .pty_manager
            .lock()
            .map_err(|e| format!("Lock error: {e}"))?;
        mgr.spawn(&shell, 80, 24).map_err(|e| e.to_string())?
    };

    // Background read loop: read PTY output, base64 encode, send via channel
    std::thread::spawn(move || {
        read_pty_output(reader, on_output);
    });

    Ok(id)
}

fn read_pty_output(mut reader: Box<dyn Read + Send>, channel: Channel<String>) {
    let mut buf = [0u8; 4096];
    let engine = base64::engine::general_purpose::STANDARD;

    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — child exited
                let _ = channel.send("__EOF__".to_string());
                break;
            }
            Ok(n) => {
                let encoded = engine.encode(&buf[..n]);
                if channel.send(encoded).is_err() {
                    // Channel closed (frontend disconnected)
                    break;
                }
            }
            Err(e) => {
                // Read error — likely PTY closed
                let _ = channel.send(format!("__ERROR__:{e}"));
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            pty_manager: Mutex::new(PtyManager::new()),
        })
        .invoke_handler(tauri::generate_handler![pty_spawn, pty_write, pty_resize])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
