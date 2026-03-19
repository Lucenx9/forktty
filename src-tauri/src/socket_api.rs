use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::oneshot;

use crate::pty_manager::PtyManager;

/// Maximum request size (1 MiB).
const MAX_REQUEST_SIZE: usize = 1_048_576;

/// Returns the default socket path, preferring XDG_RUNTIME_DIR for security.
pub fn default_socket_path() -> String {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{runtime_dir}/forktty.sock")
    } else {
        "/tmp/forktty.sock".to_string()
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Pending frontend bridge requests: request_id -> response sender.
pub type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Resolve a pending bridge request (called from Tauri command).
pub fn resolve_request(pending: &PendingRequests, id: &str, result: Value) {
    if let Ok(mut map) = pending.lock() {
        if let Some(tx) = map.remove(id) {
            let _ = tx.send(result);
        }
    }
}

/// Start the Unix socket JSON-RPC server.
pub async fn run(
    socket_path: String,
    app_handle: tauri::AppHandle,
    pty_manager: Arc<Mutex<PtyManager>>,
    pending: PendingRequests,
) {
    // Remove stale socket file
    if Path::new(&socket_path).exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind socket at {socket_path}: {e}");
            return;
        }
    };

    // Restrict socket to owner only (mode 0600)
    if let Err(e) = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("Failed to set socket permissions: {e}");
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app_handle.clone();
                let ptys = pty_manager.clone();
                let pend = pending.clone();
                tokio::spawn(async move {
                    handle_connection(stream, app, ptys, pend).await;
                });
            }
            Err(e) => {
                eprintln!("Socket accept error: {e}");
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    app: tauri::AppHandle,
    pty_manager: Arc<Mutex<PtyManager>>,
    pending: PendingRequests,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_REQUEST_SIZE {
            let resp = json!({"id": null, "ok": false, "error": {"code": "request_too_large", "message": "Request exceeds 1 MiB"}});
            let _ = write_response(&mut writer, &resp).await;
            break;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({"id": null, "ok": false, "error": {"code": "parse_error", "message": e.to_string()}});
                let _ = write_response(&mut writer, &resp).await;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let result = dispatch(method, params, &app, &pty_manager, &pending).await;

        let resp = match result {
            Ok(val) => json!({"id": id, "ok": true, "result": val}),
            Err(msg) => json!({"id": id, "ok": false, "error": {"code": "error", "message": msg}}),
        };

        if write_response(&mut writer, &resp).await.is_err() {
            break;
        }
    }
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &Value,
) -> Result<(), std::io::Error> {
    let mut bytes = serde_json::to_vec(resp)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn dispatch(
    method: &str,
    params: Value,
    app: &tauri::AppHandle,
    pty_manager: &Arc<Mutex<PtyManager>>,
    pending: &PendingRequests,
) -> Result<Value, String> {
    match method {
        // --- Direct backend handlers ---
        "system.ping" => Ok(json!("pong")),

        "surface.send_text" => {
            let pty_id = params
                .get("pty_id")
                .and_then(|v| v.as_u64())
                .ok_or("Missing pty_id")? as u32;
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing text")?;
            if text.len() > MAX_REQUEST_SIZE {
                return Err("Text exceeds 1 MiB".to_string());
            }
            let mgr = pty_manager.lock().map_err(|e| format!("Lock: {e}"))?;
            mgr.write(pty_id, text.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(json!(true))
        }

        "worktree.merge" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            crate::worktree::merge(&cwd.to_string_lossy(), name).map_err(|e| e.to_string())?;
            Ok(json!(format!("Merged '{name}'")))
        }

        // --- Bridged to frontend ---
        "workspace.list"
        | "workspace.create"
        | "workspace.select"
        | "workspace.close"
        | "surface.list"
        | "surface.split"
        | "surface.close"
        | "notification.create"
        | "notification.list"
        | "notification.clear" => bridge_to_frontend(app, pending, method, params).await,

        _ => Err(format!("Unknown method: {method}")),
    }
}

/// Forward a request to the frontend via Tauri events, wait for response.
async fn bridge_to_frontend(
    app: &tauri::AppHandle,
    pending: &PendingRequests,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    use tauri::Emitter;

    let req_id = format!("sr-{}", NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();

    {
        let mut map = pending.lock().map_err(|e| format!("Lock: {e}"))?;
        map.insert(req_id.clone(), tx);
    }

    app.emit(
        "socket-request",
        json!({ "id": req_id, "method": method, "params": params }),
    )
    .map_err(|e| format!("Emit failed: {e}"))?;

    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(result)) => {
            if let Some(err) = result.get("error").and_then(|e| e.as_str()) {
                Err(err.to_string())
            } else {
                Ok(result.get("result").cloned().unwrap_or(result))
            }
        }
        Ok(Err(_)) => Err("Bridge channel closed".to_string()),
        Err(_) => {
            // Clean up timed-out request
            if let Ok(mut map) = pending.lock() {
                map.remove(&req_id);
            }
            Err("Request timed out (frontend not responding)".to_string())
        }
    }
}
